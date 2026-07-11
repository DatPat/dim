import { useCallback, useContext, useEffect } from "react";
import { useDispatch, useSelector } from "react-redux";

import BackwardIcon from "../../../../assets/Icons/Backward";
import { updateVideo } from "../../../../actions/video";
import { VideoPlayerContext } from "../../Context";
import { UnfocusableButton } from "Components/unfocusableButton";

function VideoActionSeekBack() {
  const dispatch = useDispatch();

  const { video } = useSelector((store) => ({
    video: store.video,
  }));

  const wt = useSelector((store) => store.watchTogether);
  const isWtNonHost = wt.externalSync ? wt.externalIsGuest : (wt.isInRoom && wt.controlMode === "host_only" && !wt.isHost);

  const { seekTo } = useContext(VideoPlayerContext);

  const seekBackward = useCallback(() => {
    if (isWtNonHost) return;
    dispatch(
      updateVideo({
        idleCount: 0,
      })
    );

    if (video.currentTime - 15 <= 0) {
      seekTo(0);
    } else {
      seekTo(video.currentTime - 15);
    }
  }, [dispatch, seekTo, video.currentTime, isWtNonHost]);

  const handleKeyDown = useCallback(
    (e) => {
      if (isWtNonHost) return;
      if (e.key === "ArrowLeft") {
        seekBackward();
      }
    },
    [seekBackward, isWtNonHost]
  );

  useEffect(() => {
    document.addEventListener("keydown", handleKeyDown);

    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleKeyDown]);

  const className = `backward${isWtNonHost ? " wtDisabled" : ""}`;

  return (
    <UnfocusableButton onClick={seekBackward} className={className}>
      <BackwardIcon />
    </UnfocusableButton>
  );
}

export default VideoActionSeekBack;
