import { useCallback, useEffect, useContext } from "react";
import { useDispatch, useSelector } from "react-redux";

import PlayIcon from "../../../../assets/Icons/Play";
import PauseIcon from "../../../../assets/Icons/Pause";

import { WebSocketContext } from "../../../../Components/WS";
import { VideoPlayerContext } from "../../Context";

import { updateVideo } from "../../../../actions/video";

function VideoActionPlayPause() {
  const dispatch = useDispatch();
  const ws = useContext(WebSocketContext);

  const { player, videoRef } = useContext(VideoPlayerContext);

  const { video } = useSelector((store) => ({
    video: store.video,
  }));

  const wt = useSelector((store) => store.watchTogether);
  const isWtNonHost = wt.externalSync ? wt.externalIsGuest : (wt.isInRoom && wt.controlMode === "host_only" && !wt.isHost);
  const canBroadcast = wt.isInRoom && wt.roomCode &&
    (wt.isHost || wt.controlMode === "egalitarian");

  const play = useCallback(() => {
    if (isWtNonHost) return;
    dispatch(
      updateVideo({
        idleCount: 0,
      })
    );

    player.play();

    if (ws && canBroadcast && videoRef?.current) {
      ws.send(JSON.stringify({
        type: "watch_together_play",
        room_code: wt.roomCode,
        position: videoRef.current.currentTime,
      }));
    }
  }, [dispatch, player, isWtNonHost, ws, canBroadcast, wt.roomCode, videoRef]);

  const pause = useCallback(() => {
    if (isWtNonHost) return;
    dispatch(
      updateVideo({
        idleCount: 0,
      })
    );

    player.pause();

    if (ws && canBroadcast && videoRef?.current) {
      ws.send(JSON.stringify({
        type: "watch_together_pause",
        room_code: wt.roomCode,
        position: videoRef.current.currentTime,
      }));
    }
  }, [dispatch, player, isWtNonHost, ws, canBroadcast, wt.roomCode, videoRef]);

  const handleKeyDown = useCallback(
    (e) => {
      if (isWtNonHost) return;
      if (e.key !== " ") return;
      if (e.target !== document.body) return;

      player.isPaused() ? play() : pause();
    },
    [pause, play, player, isWtNonHost]
  );

  useEffect(() => {
    document.addEventListener("keydown", handleKeyDown);

    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleKeyDown]);

  const className = `playpause${isWtNonHost ? " wtDisabled wtDisabledTooltip" : ""}`;

  // Decide play-vs-pause from the ELEMENT, not redux: if the two ever
  // disagree (a paused element with redux thinking "playing"), the
  // redux-driven choice calls pause() on an already-paused element, which
  // fires no event — so redux never corrects and the button stays dead.
  // The element is ground truth and self-heals in one click.
  const toggle = useCallback(() => {
    const elPaused = videoRef?.current ? videoRef.current.paused : video.paused;
    elPaused ? play() : pause();
  }, [videoRef, video.paused, play, pause]);

  return (
    <button onClick={toggle} className={className}>
      {video.paused ? <PlayIcon /> : <PauseIcon />}
    </button>
  );
}

export default VideoActionPlayPause;
