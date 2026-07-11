import { useCallback, useContext, useEffect, useRef } from "react";
import { useDispatch, useSelector } from "react-redux";

import { WebSocketContext } from "../../../Components/WS";
import { VideoPlayerContext } from "../Context";

import {
  wtSyncPlayback,
  wtUpdateParticipants,
  wtAddChatMessage,
  wtRoomDestroyed,
  wtSetBuffering,
  wtSetReady,
  wtModeChanged,
} from "../../../actions/watchTogether";
import { updateVideo } from "../../../actions/video";

const DRIFT_HARD_SEEK_THRESHOLD = 3;
const DRIFT_SOFT_THRESHOLD = 0.5;
const DRIFT_CHECK_INTERVAL = 2000;
const RATE_ADJUST_FAST = 1.03;
const RATE_ADJUST_SLOW = 0.97;

// How long after a sync-triggered seek to suppress buffering events (ms).
const SYNC_SUPPRESS_MS = 3000;

// How long after joining a room to suppress buffering events (ms).
const JOIN_GRACE_MS = 5000;

function SyncController() {
  const dispatch = useDispatch();
  const ws = useContext(WebSocketContext);
  const { player, videoRef } = useContext(VideoPlayerContext);

  const wtState = useSelector((store) => store.watchTogether);
  const { roomCode, isHost, isInRoom } = wtState;
  const currentUsername = useSelector((store) => store.user?.info?.username);
  const auth = useSelector((store) => store.auth);

  const driftIntervalRef = useRef(null);
  const syncingUntilRef = useRef(0);
  const joinGraceUntilRef = useRef(0);
  const initialSyncDoneRef = useRef(false);
  // Last sync command that arrived before the player was ready; replayed
  // once the player exists so commands during manifest load aren't lost.
  const pendingSyncRef = useRef(null);

  // Effect callbacks read the latest room state through this ref so the
  // effects below don't have to depend on the whole wtState object (a new
  // object on every WT message — depending on it made the initial-sync
  // effect re-run and cancel its own pending host broadcast).
  const wtStateRef = useRef(wtState);
  useEffect(() => {
    wtStateRef.current = wtState;
  });

  // Reset the initial sync flag when leaving the room or when a new player
  // is created (episode change while staying in the room).
  useEffect(() => {
    if (!isInRoom) {
      initialSyncDoneRef.current = false;
    }
  }, [isInRoom]);
  useEffect(() => {
    initialSyncDoneRef.current = false;
  }, [player]);

  // ---------------------------------------------------------------
  // INITIAL SYNC: When joining a room and the player is ready,
  // immediately seek to the room's position.
  //
  // For the HOST: send current position to the server so the room
  //   state reflects reality.
  // For JOINERS: seek + play/pause to match the room state from
  //   the join response (stored in Redux).
  // ---------------------------------------------------------------
  useEffect(() => {
    if (!isInRoom || !player || !videoRef?.current || !roomCode) return;
    if (initialSyncDoneRef.current) return;

    initialSyncDoneRef.current = true;
    joinGraceUntilRef.current = Date.now() + JOIN_GRACE_MS;
    syncingUntilRef.current = Date.now() + SYNC_SUPPRESS_MS;

    if (isHost) {
      // Host: tell the server where we are. Deliberately NOT cancelled on
      // effect cleanup — the guard inside re-checks room membership.
      if (ws) {
        const video = videoRef.current;
        setTimeout(() => {
          if (!wtStateRef.current.isInRoom) return;
          const position = video.currentTime || 0;
          const action = video.paused ? "pause" : "play";
          ws.send(
            JSON.stringify({
              type: `watch_together_${action}`,
              room_code: roomCode,
              position,
            })
          );
        }, 500);
      }
    } else {
      // Joiner: seek to the room's position from the join response
      const room = wtStateRef.current;
      const serverPos = room.syncPosition || 0;
      const elapsed = room.lastSyncServerTime
        ? (Date.now() - room.lastSyncServerTime) / 1000
        : 0;
      const targetPos =
        room.playbackState === "playing"
          ? serverPos + Math.max(0, elapsed)
          : serverPos;

      player.seek(targetPos);
      if (room.playbackState === "playing") {
        videoRef.current.play().catch(() => {});
        dispatch(updateVideo({ paused: false }));
      } else {
        videoRef.current.pause();
        // pause() on an element that never started fires no event — mirror
        // into redux so the play/pause button isn't rendered inverted.
        dispatch(updateVideo({ paused: true }));
      }
    }
  }, [isInRoom, player, videoRef, roomCode, isHost, ws, dispatch]);

  // ---------------------------------------------------------------
  // Applying a remote sync command to the local player.
  // Kept in a ref so both the live WS handler and the pending-command
  // replay below share one implementation.
  // ---------------------------------------------------------------
  const applySyncRef = useRef(() => {});
  useEffect(() => {
    applySyncRef.current = (data) => {
      if (!player || !videoRef?.current) return;

      syncingUntilRef.current = Date.now() + SYNC_SUPPRESS_MS;

      const latencyOffset = (Date.now() - data.server_time_ms) / 1000;

      // Mirror the commanded state into redux explicitly: play() on an
      // already-playing element (or pause() on a paused one) fires no
      // media event, so the dash.js event handlers alone can leave redux
      // contradicting the element — which renders the wrong play/pause icon.
      if (data.action === "play") {
        const adjustedPos = data.position + Math.max(0, latencyOffset);
        player.seek(adjustedPos);
        videoRef.current.play().catch(() => {});
        dispatch(updateVideo({ paused: false }));
      } else if (data.action === "pause") {
        player.seek(data.position);
        videoRef.current.pause();
        dispatch(updateVideo({ paused: true }));
      } else if (data.action === "seek") {
        // Only advance by transit latency while the room is playing —
        // a paused seek target must land exactly where it was aimed.
        const playing = !videoRef.current.paused;
        const adjustedPos =
          data.position + (playing ? Math.max(0, latencyOffset) : 0);
        player.seek(adjustedPos);
      }
    };
  }, [player, videoRef]);

  // Replay a sync command that arrived before the player existed.
  useEffect(() => {
    if (!player || !videoRef?.current || !pendingSyncRef.current) return;
    const data = pendingSyncRef.current;
    pendingSyncRef.current = null;
    applySyncRef.current(data);
  }, [player, videoRef]);

  // ---------------------------------------------------------------
  // WebSocket message handler
  // ---------------------------------------------------------------
  const handleWsMessage = useCallback(
    (e) => {
      let data;
      try {
        data = JSON.parse(e.data);
      } catch {
        return;
      }

      switch (data.type) {
        case "EventWatchTogetherSync": {
          if (data.room_code !== roomCode) return;
          dispatch(wtSyncPlayback(data));

          if (!player || !videoRef?.current) {
            // Player not ready yet (manifest loading) — queue the command
            // so it can be replayed instead of silently dropped.
            pendingSyncRef.current = data;
            return;
          }

          applySyncRef.current(data);
          break;
        }
        case "EventWatchTogetherRoomUpdate": {
          if (data.room_code !== roomCode) return;
          dispatch(
            wtUpdateParticipants({
              participants: data.participants,
              currentUsername,
              playback_state: data.playback_state,
              playback_position: data.playback_position,
              server_time_ms: data.server_time_ms,
            })
          );
          break;
        }
        case "EventWatchTogetherChat": {
          if (data.room_code !== roomCode) return;
          dispatch(
            wtAddChatMessage({
              user_id: data.user_id,
              username: data.username,
              text: data.text,
              timestamp_ms: data.timestamp_ms,
            })
          );
          break;
        }
        case "EventWatchTogetherRoomDestroyed": {
          if (data.room_code !== roomCode) return;
          dispatch(wtRoomDestroyed({ reason: data.reason }));
          break;
        }
        case "EventWatchTogetherBuffering": {
          if (data.room_code !== roomCode) return;
          dispatch(
            wtSetBuffering({
              user_id: data.user_id,
              is_buffering: data.is_buffering,
            })
          );
          break;
        }
        case "EventWatchTogetherReady": {
          if (data.room_code !== roomCode) return;
          dispatch(
            wtSetReady({
              user_id: data.user_id,
              is_ready: data.is_ready,
              // Lets the reducer tell "my toggle" apart from someone
              // else's — only mine may move my Ready button.
              currentUsername,
            })
          );
          break;
        }
        case "EventWatchTogetherModeChanged": {
          if (data.room_code !== roomCode) return;
          dispatch(
            wtModeChanged({
              control_mode: data.control_mode,
            })
          );
          break;
        }
        default:
          break;
      }
    },
    [dispatch, roomCode, player, videoRef, currentUsername]
  );

  useEffect(() => {
    if (!ws || !isInRoom) return;

    ws.addEventListener("message", handleWsMessage);
    return () => ws.removeEventListener("message", handleWsMessage);
  }, [ws, isInRoom, handleWsMessage]);

  // NOTE: Sync broadcasting (play/pause/seek) is handled directly in the UI
  // action handlers (Index.jsx, PlayPause.jsx) rather than via native video
  // element event listeners. Native events fire for both user actions AND
  // internal dash.js operations (buffer resets, timeline corrections), so
  // listening to them causes feedback loops where internal player state
  // changes get amplified through the sync system.

  // All participants: send buffering status (with anti-feedback suppression)
  useEffect(() => {
    if (!videoRef?.current || !ws || !roomCode) return;

    const video = videoRef.current;

    const shouldSuppressBuffering = () => {
      const now = Date.now();
      return now < syncingUntilRef.current || now < joinGraceUntilRef.current;
    };

    const onWaiting = () => {
      if (shouldSuppressBuffering()) return;
      ws.send(
        JSON.stringify({
          type: "watch_together_buffering",
          room_code: roomCode,
          is_buffering: true,
        })
      );
    };

    const onPlaying = () => {
      if (shouldSuppressBuffering()) return;
      ws.send(
        JSON.stringify({
          type: "watch_together_buffering",
          room_code: roomCode,
          is_buffering: false,
        })
      );
    };

    video.addEventListener("waiting", onWaiting);
    video.addEventListener("playing", onPlaying);

    return () => {
      video.removeEventListener("waiting", onWaiting);
      video.removeEventListener("playing", onPlaying);
    };
  }, [videoRef, ws, roomCode]);

  // Drift correction every 2s for everyone except the host. This must run
  // in egalitarian rooms too — `canControl` is true for every participant
  // there, and gating on it disabled drift correction for the exact mode
  // that needs it most (everyone free-runs and slowly drifts apart).
  useEffect(() => {
    if (isHost || !isInRoom || !player || !videoRef?.current || !roomCode) {
      if (driftIntervalRef.current) {
        clearInterval(driftIntervalRef.current);
        driftIntervalRef.current = null;
      }
      if (videoRef?.current) {
        videoRef.current.playbackRate = 1.0;
      }
      return;
    }

    driftIntervalRef.current = setInterval(async () => {
      try {
        const res = await fetch(`/api/v1/watch-together/rooms/${roomCode}`, {
          headers: { Authorization: auth.token },
        });

        if (!res.ok) return;

        const room = await res.json();
        const serverPos = room.playback_position;
        const elapsed = (Date.now() - room.server_time_ms) / 1000;
        const expectedPos =
          room.playback_state === "playing" ? serverPos + elapsed : serverPos;
        const localPos = videoRef.current.currentTime;
        const drift = localPos - expectedPos;
        const absDrift = Math.abs(drift);

        if (absDrift > DRIFT_HARD_SEEK_THRESHOLD) {
          syncingUntilRef.current = Date.now() + SYNC_SUPPRESS_MS;
          videoRef.current.playbackRate = 1.0;
          player.seek(expectedPos);
        } else if (absDrift > DRIFT_SOFT_THRESHOLD) {
          videoRef.current.playbackRate =
            drift > 0 ? RATE_ADJUST_SLOW : RATE_ADJUST_FAST;
        } else {
          videoRef.current.playbackRate = 1.0;
        }
      } catch {
        // ignore drift check errors
      }
    }, DRIFT_CHECK_INTERVAL);

    return () => {
      if (driftIntervalRef.current) {
        clearInterval(driftIntervalRef.current);
        driftIntervalRef.current = null;
      }
      if (videoRef?.current) {
        videoRef.current.playbackRate = 1.0;
      }
    };
  }, [isHost, isInRoom, player, videoRef, roomCode, auth.token]);

  return null;
}

export default SyncController;
