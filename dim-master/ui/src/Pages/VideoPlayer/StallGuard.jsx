import { useContext, useEffect, useRef } from "react";
import { useDispatch, useSelector } from "react-redux";

import { VideoPlayerContext } from "./Context";
import { setGID, updateVideo } from "../../actions/video";

// Poll cadence for the stall detector.
const TICK_MS = 2000;
// Playhead frozen this long WITH data buffered ahead → decoder/gap stall.
const NUDGE_AFTER_S = 8;
// Playhead frozen this long no matter what → rebuild the whole stream
// session (covers dead server sessions, e.g. reaped after a long pause,
// and mid-stream encoder fallbacks the SourceBuffer can't digest).
const REBUILD_AFTER_S = 30;
// Don't rebuild more than once per this window (avoid loops on hard failures).
const REBUILD_COOLDOWN_MS = 3 * 60 * 1000;

/**
 * Watchdog for rare playback freezes.
 *
 * A frozen playhead while the element claims to be playing has a handful of
 * rare causes (unjumped timeline gap, decoder stall on a mid-stream encoder
 * switch, the server session getting garbage-collected during a long pause).
 * None of them produce an error event, so without this the video just sits
 * frozen forever. Recovery escalates: nudge-seek first (forces dash.js to
 * re-evaluate the buffer/gap), then a full stream-session rebuild.
 */
function StallGuard() {
  const dispatch = useDispatch();
  const { player, videoRef } = useContext(VideoPlayerContext);
  const gid = useSelector((store) => store.video.gid);
  const canPlay = useSelector((store) => store.video.canPlay);
  const errorState = useSelector((store) => store.video.error);

  const playerRef = useRef(player);
  useEffect(() => {
    playerRef.current = player;
  }, [player]);

  const lastTimeRef = useRef(-1);
  const stalledSecsRef = useRef(0);
  const nudgesRef = useRef(0);
  const lastRebuildRef = useRef(0);
  // Position to restore after a session rebuild.
  const resumeToRef = useRef(null);

  useEffect(() => {
    const rebuild = (resumeAt) => {
      lastRebuildRef.current = Date.now();
      stalledSecsRef.current = 0;
      nudgesRef.current = 0;
      resumeToRef.current = resumeAt;
      console.warn(
        `[stallguard] rebuilding stream session (resume at ${resumeAt.toFixed(1)}s)`
      );
      // Clearing the gid makes the player page fetch a brand-new virtual
      // manifest (new transcode sessions server-side) and rebuild dash.js.
      dispatch(updateVideo({ error: null }));
      dispatch(setGID(null));
    };

    const interval = setInterval(() => {
      const v = videoRef?.current;
      const p = playerRef.current;

      // A fatal player error is another face of the same problem — the old
      // session is unusable. Recover the same way. A stale error dispatched
      // by the torn-down player right after OUR rebuild is just cleared.
      if (errorState) {
        if (Date.now() - lastRebuildRef.current < 90 * 1000) {
          console.warn("[stallguard] clearing stale player error after rebuild");
          dispatch(updateVideo({ error: null }));
        } else if (Date.now() - lastRebuildRef.current > REBUILD_COOLDOWN_MS) {
          rebuild(v ? v.currentTime : Math.max(0, lastTimeRef.current));
        }
        return;
      }

      if (!v || !p) return;

      // After a rebuild: once the new session can play, jump back to where
      // the user was.
      if (resumeToRef.current != null) {
        if (canPlay && v.readyState >= 2) {
          const target = resumeToRef.current;
          resumeToRef.current = null;
          console.log(`[stallguard] resuming at ${target.toFixed(1)}s after rebuild`);
          try {
            p.seek(target);
            v.play().catch(() => {});
          } catch {
            // player mid-teardown; nothing to do
          }
        }
        return;
      }

      // Only watch an actively-playing video.
      if (v.paused || v.ended || v.seeking || !canPlay) {
        stalledSecsRef.current = 0;
        nudgesRef.current = 0;
        lastTimeRef.current = v.currentTime;
        return;
      }

      if (v.currentTime !== lastTimeRef.current) {
        lastTimeRef.current = v.currentTime;
        stalledSecsRef.current = 0;
        nudgesRef.current = 0;
        return;
      }

      stalledSecsRef.current += TICK_MS / 1000;

      // Is there data buffered beyond the playhead? If so the network is
      // fine and the decoder/timeline is stuck — a nudge-seek usually
      // unsticks it (and triggers dash.js gap handling).
      let bufferedAhead = false;
      for (let i = 0; i < v.buffered.length; i++) {
        if (
          v.buffered.start(i) <= v.currentTime + 0.5 &&
          v.buffered.end(i) > v.currentTime + 1.5
        ) {
          bufferedAhead = true;
          break;
        }
      }

      if (
        bufferedAhead &&
        stalledSecsRef.current >= NUDGE_AFTER_S * (nudgesRef.current + 1) &&
        nudgesRef.current < 2
      ) {
        nudgesRef.current += 1;
        console.warn(
          `[stallguard] playhead frozen ${stalledSecsRef.current}s with data buffered — nudge #${nudgesRef.current}`
        );
        try {
          p.seek(v.currentTime + 0.1);
        } catch {
          // ignore
        }
        return;
      }

      if (
        stalledSecsRef.current >= REBUILD_AFTER_S &&
        Date.now() - lastRebuildRef.current > REBUILD_COOLDOWN_MS
      ) {
        console.warn(
          `[stallguard] playhead frozen ${REBUILD_AFTER_S}s — rebuilding`
        );
        rebuild(v.currentTime);
      }
    }, TICK_MS);

    return () => clearInterval(interval);
  }, [dispatch, videoRef, canPlay, gid, errorState]);

  return null;
}

export default StallGuard;
