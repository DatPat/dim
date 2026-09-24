import { useCallback, useRef } from "react";
import { useDispatch, useStore } from "react-redux";
import { useHistory } from "react-router-dom";
import { changeRoomMedia } from "../../../actions/watchTogether";

export default function useEpisodeNavigation(fileID) {
  const dispatch = useDispatch();
  const store = useStore();
  const history = useHistory();
  const pending = useRef(false);

  return useCallback(async (nextFileID) => {
    if (pending.current || String(fileID) === String(nextFileID)) return;
    const room = store.getState().watchTogether;
    if (room.isInRoom && !room.externalSync) {
      // Guests follow the host's room update, including in egalitarian rooms.
      if (!room.isHost || String(room.mediaFileId) !== String(fileID)) return;
      pending.current = true;
      try {
        await dispatch(changeRoomMedia(nextFileID, fileID));
      } finally {
        pending.current = false;
      }
    } else {
      if (room.externalSync && room.externalIsGuest) return;
      history.replace(`/play/${nextFileID}`, { from: history.location.pathname });
    }
  }, [dispatch, store, history, fileID]);
}
