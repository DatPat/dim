import { useCallback, useEffect, useState } from "react";
import { useHistory } from "react-router-dom";
import { useSelector } from "react-redux";

import Button from "Components/Misc/Button";

import "./Index.scss";

// Shown in place of the next-episode button when the episode being played
// is the last one available — offers a way back to the show page.
function ReturnToShow(props) {
  const { showId, showAfter } = props;
  const video = useSelector((store) => store.video);

  const history = useHistory();
  const [visible, setVisible] = useState(true);

  useEffect(() => {
    setVisible(video.idleCount <= 2 && video.currentTime >= showAfter);
  }, [video.idleCount, video.currentTime, showAfter]);

  const goToShow = useCallback(() => {
    history.push(`/media/${showId}`);
  }, [history, showId]);

  return (
    <div className={`nextVideoOverlay ${visible}`}>
      <Button type="icon" onClick={goToShow}>
        <p>Return to Show</p>
      </Button>
    </div>
  );
}

export default ReturnToShow;
