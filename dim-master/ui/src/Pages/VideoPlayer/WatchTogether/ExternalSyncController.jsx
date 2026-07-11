import { useContext, useEffect, useRef } from "react";
import { useSelector } from "react-redux";
import { useParams } from "react-router-dom";
import { skipToken } from "@reduxjs/toolkit/query/react";

import { VideoPlayerContext } from "../Context";
import { useExternalSync } from "../../../Controllers/ExternalSyncContext";
import { useGetMediaFilesQuery } from "../../../api/v1/media";

const DRIFT_HARD_SEEK_THRESHOLD = 10;
const DRIFT_SOFT_THRESHOLD = 3;
const STATE_REPORT_INTERVAL = 2000;
const RATE_ADJUST_FAST = 1.04;
const RATE_ADJUST_SLOW = 0.96;

/**
 * Syncs local video playback with the external Syncplay server.
 *
 * Key design: we track the host's desired state (position + paused) in a ref.
 * When the local player becomes ready (canPlay), we apply the host state.
 * This solves the race where server state arrives before the manifest loads.
 *
 * For guests (externalIsGuest), outgoing state reports echo the host's
 * desired state rather than the local state, and user-initiated play/pause
 * events are suppressed.
 */
function ExternalSyncController() {
  const { player, videoRef } = useContext(VideoPlayerContext);
  const externalSync = useSelector((store) => store.watchTogether.externalSync);
  const isGuest = useSelector((store) => store.watchTogether.externalIsGuest);
  const video = useSelector((store) => store.video);
  const syncCtx = useExternalSync();
  const { clientRef, onStateUpdate } = syncCtx || {};

  const reportIntervalRef = useRef(null);
  const params = useParams();

  const playerRef = useRef(player);
  useEffect(() => { playerRef.current = player; }, [player]);

  // Keep isGuest in a ref so the onStateUpdate closure always has the latest.
  const isGuestRef = useRef(isGuest);
  useEffect(() => { isGuestRef.current = isGuest; }, [isGuest]);

  // Track the host's desired playback state so we can apply it
  // when the player becomes ready (manifest loaded, can play).
  const hostStateRef = useRef({ position: 0, paused: true });
  // True once we've applied the first host "play" command.
  const hasAutoPlayedRef = useRef(false);
  // Suppress outgoing reports after applying server state (anti-echo).
  const suppressUntilRef = useRef(0);

  // Fetch the actual filename from the media files API
  const { data: mediaFiles } = useGetMediaFilesQuery(
    video.mediaID ? video.mediaID : skipToken
  );
  const fileID = parseInt(params.fileID);
  const currentFile = mediaFiles?.find((f) => f.id === fileID);

  // A new file (host changed episodes) needs a fresh autoplay one-shot.
  useEffect(() => {
    hasAutoPlayedRef.current = false;
  }, [fileID]);

  // Subscribe to server state updates
  useEffect(() => {
    if (!externalSync) {
      return;
    }

    const connectedAt = Date.now();
    const GRACE_MS = 3000;

    const unsubscribe = onStateUpdate((position, paused, doSeek) => {
      // Always track the latest host state (used for guest autoplay on canplay)
      hostStateRef.current = { position, paused };

      // The HOST is authoritative — server state must never override the
      // host's local playback.
      if (!isGuestRef.current) return;

      // --- Guest-only logic below ---
      const p = playerRef.current;
      if (!p || !videoRef?.current) {
        return;
      }

      const localPos = videoRef.current.currentTime;
      const localPaused = videoRef.current.paused;

      // Grace period — ignore early server states with position=0
      if (Date.now() - connectedAt < GRACE_MS && position < 1) {
        return;
      }

      // Never seek backward to near-zero once we've moved past the start.
      if (position < 1 && localPos > 2) {
        return;
      }

      // Suppress outgoing reports
      suppressUntilRef.current = Date.now() + 2000;

      if (doSeek) {
        p.seek(position);
      }

      if (paused && !localPaused) {
        videoRef.current.pause();
        p.seek(position);
      } else if (!paused && localPaused) {
        p.seek(position);
        videoRef.current.play().catch(() => {
          videoRef.current.muted = true;
          videoRef.current.play().catch(() => {});
        });
      } else if (!paused) {
        // Both playing — drift correction
        const drift = localPos - position;
        const absDrift = Math.abs(drift);

        if (absDrift > DRIFT_HARD_SEEK_THRESHOLD) {
          videoRef.current.playbackRate = 1.0;
          p.seek(position);
        } else if (absDrift > DRIFT_SOFT_THRESHOLD) {
          videoRef.current.playbackRate =
            drift > 0 ? RATE_ADJUST_SLOW : RATE_ADJUST_FAST;
        } else {
          videoRef.current.playbackRate = 1.0;
        }
      }
    });

    return unsubscribe;
  }, [externalSync, onStateUpdate, videoRef]);

  // For guests: mute the video element immediately so autoplay isn't blocked
  // by browser policy. The user can unmute manually via volume controls.
  useEffect(() => {
    if (!isGuest || !videoRef?.current) return;
    videoRef.current.muted = true;
  }, [isGuest, videoRef]);

  // Auto-play when the player becomes ready (canPlay) and the host is playing.
  // This handles the race where the host starts before our manifest loads.
  useEffect(() => {
    if (!externalSync || !videoRef?.current) return;

    const vid = videoRef.current;

    const tryAutoPlay = () => {
      if (hasAutoPlayedRef.current) return;
      if (hostStateRef.current.paused) return;
      if (!isGuestRef.current) return; // only guests autoplay

      hasAutoPlayedRef.current = true;
      const pos = hostStateRef.current.position;

      vid.muted = true; // ensure muted for autoplay policy
      const p = playerRef.current;
      if (p && pos > 0) {
        p.seek(pos);
      }
      vid.play().catch(() => {});
    };

    vid.addEventListener("canplay", tryAutoPlay);
    // Also try on loadeddata in case canplay already fired
    vid.addEventListener("loadeddata", tryAutoPlay);
    return () => {
      vid.removeEventListener("canplay", tryAutoPlay);
      vid.removeEventListener("loadeddata", tryAutoPlay);
    };
  }, [externalSync, videoRef]);

  // Periodically report local state to the server.
  // Guests never send independent reports — the SyncplayClient's auto-respond
  // echoes the server's authoritative state, which is sufficient.
  // Only the host (non-guest) sends authoritative position updates.
  useEffect(() => {
    if (!externalSync || isGuest) return;

    reportIntervalRef.current = setInterval(() => {
      if (!videoRef?.current || !clientRef.current) return;

      if (Date.now() < suppressUntilRef.current) return;

      const pos = videoRef.current.currentTime;
      if (pos < 0.5 && !videoRef.current.paused) return;
      clientRef.current.reportState(pos, videoRef.current.paused, false);
    }, STATE_REPORT_INTERVAL);

    return () => {
      if (reportIntervalRef.current) {
        clearInterval(reportIntervalRef.current);
        reportIntervalRef.current = null;
      }
      if (videoRef?.current) {
        videoRef.current.playbackRate = 1.0;
      }
    };
  }, [externalSync, isGuest, clientRef, videoRef]);

  // Send file info when the actual filename becomes available.
  // Both host and guest send this — the Syncplay server requires all
  // clients to have a file set to stay connected. For guests, we send
  // the file WITHOUT the [dim:] tag so we don't get mistaken for the host.
  useEffect(() => {
    if (!clientRef.current || !currentFile) return;

    const fileName = currentFile.target_file.split(/\/|\\/g).pop() || "Unknown";
    const duration = currentFile.duration || 0;
    if (isGuest) {
      // Guest: announce the file WITHOUT marking ourselves host — a guest
      // that flips to host stops following the real host and echoes stale
      // state (position 0, paused) forever.
      clientRef.current.setFile(fileName, duration, 0, 0, false);
    } else {
      // Host: send with dim tag so guests can find the file
      clientRef.current.setFile(fileName, duration, 0, fileID, true);
    }
  }, [clientRef, currentFile, fileID, isGuest]);

  // For guests: block user-initiated play/pause from propagating to server.
  // Only the host forwards native video events as authoritative state.
  useEffect(() => {
    if (!videoRef?.current || !clientRef.current || !externalSync) return;
    if (isGuest) return;

    const vid = videoRef.current;

    const onPlay = () => {
      if (Date.now() < suppressUntilRef.current) return;
      const pos = vid.currentTime;
      if (pos < 0.5) return;
      clientRef.current?.reportState(pos, false, false);
    };

    const onPause = () => {
      if (Date.now() < suppressUntilRef.current) return;
      const pos = vid.currentTime;
      if (pos < 0.5) return;
      clientRef.current?.reportState(pos, true, false);
    };

    // Host seeks MUST be reported with doSeek — the server ignores large
    // position jumps that aren't marked as seeks (they'd otherwise be
    // indistinguishable from a stale/broken client), so without this a host
    // seek never reaches the room.
    const onSeeked = () => {
      if (Date.now() < suppressUntilRef.current) return;
      clientRef.current?.reportState(vid.currentTime, vid.paused, true);
    };

    vid.addEventListener("play", onPlay);
    vid.addEventListener("pause", onPause);
    vid.addEventListener("seeked", onSeeked);

    return () => {
      vid.removeEventListener("play", onPlay);
      vid.removeEventListener("pause", onPause);
      vid.removeEventListener("seeked", onSeeked);
    };
  }, [videoRef, clientRef, externalSync, isGuest]);

  return null;
}

export default ExternalSyncController;
