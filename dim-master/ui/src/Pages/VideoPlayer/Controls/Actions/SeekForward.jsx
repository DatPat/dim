import { useCallback, useContext, useEffect } from "react";
import { useDispatch, useSelector } from "react-redux";

import ForwardIcon from "../../../../assets/Icons/Forward";
import { updateVideo } from "../../../../actions/video";
import { VideoPlayerContext } from "../../Context";
import { UnfocusableButton } from "Components/unfocusableButton";

function VideoActionSeekForward() {
  const dispatch = useDispatch();

  const { video } = useSelector((store) => ({
    video: store.video,
  }));

  const wt = useSelector((store) => store.watchTogether);
  const isWtNonHost = wt.externalSync ? wt.externalIsGuest : (wt.isInRoom && wt.controlMode === "host_only" && !wt.isHost);

  const { seekTo } = useContext(VideoPlayerContext);

  const seekForward = useCallback(() => {
    if (isWtNonHost) return;
    dispatch(
      updateVideo({
        idleCount: 0,
      })
    );

    if (video.currentTime + 15 >= video.duration) {
      seekTo(video.duration);
    } else {
      seekTo(video.currentTime + 15);
    }
  }, [dispatch, seekTo, video.currentTime, video.duration, isWtNonHost]);

  const handleKeyDown = useCallback(
    (e) => {
      if (isWtNonHost) return;
      if (e.key === "ArrowRight") {
        seekForward();
      }
    },
    [seekForward, isWtNonHost]
  );

  useEffect(() => {
    document.addEventListener("keydown", handleKeyDown);

    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleKeyDown]);

  const className = `forward${isWtNonHost ? " wtDisabled" : ""}`;

  return (
    <UnfocusableButton onClick={seekForward} className={className}>
      <ForwardIcon />
    </UnfocusableButton>
  );
}

export default VideoActionSeekForward;
