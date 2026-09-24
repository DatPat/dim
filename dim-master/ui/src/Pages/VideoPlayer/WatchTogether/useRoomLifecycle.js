import { useEffect, useRef } from "react";
import { useSelector, useStore } from "react-redux";
import { useHistory, useLocation } from "react-router-dom";
import { joinRoom, leaveRoom } from "../../../actions/watchTogether";

export function removeRoomFromUrl(history) {
  const location = history.location;
  const query = new URLSearchParams(location.search);
  if (!query.has("room")) return;
  query.delete("room");
  history.replace({ ...location, search: query.toString() ? `?${query}` : "" });
}

export default function useRoomLifecycle(fileID) {
  const store = useStore();
  const history = useHistory();
  const location = useLocation();
  const code = new URLSearchParams(location.search).get("room");
  const mediaFileId = useSelector((state) => state.watchTogether.mediaFileId);
  const previousMedia = useRef(mediaFileId);

  // Join on a URL change, not on a membership change (including Leave).
  useEffect(() => {
    if (!code) return;
    const current = store.getState().watchTogether;
    if (current.isInRoom && current.roomCode === code) return;
    if (current.isInRoom && !current.externalSync) store.dispatch(leaveRoom(current.roomCode));
    let cancelled = false;
    store.dispatch(joinRoom(code)).then((room) => {
      if (cancelled) {
        if (room && store.getState().watchTogether.roomCode === room.code) {
          store.dispatch(leaveRoom(room.code));
        }
        return;
      }
      if (room && history.location.pathname !== `/play/${room.media_file_id}`) {
        history.replace(`/play/${room.media_file_id}?room=${room.code}`);
      }
    });
    return () => { cancelled = true; };
  }, [code, store, history]);

  // A server media update moves the whole room. Unrelated local navigation
  // still leaves it, so the replacement player cannot control the old room.
  useEffect(() => {
    const room = store.getState().watchTogether;
    const mediaChanged = previousMedia.current !== mediaFileId;
    previousMedia.current = mediaFileId;
    if (room.isInRoom && !room.externalSync && String(room.mediaFileId) !== String(fileID)) {
      if (mediaChanged) {
        history.replace(`/play/${room.mediaFileId}?room=${room.roomCode}`);
        return;
      }
      removeRoomFromUrl(history);
      store.dispatch(leaveRoom(room.roomCode));
    }
  }, [fileID, mediaFileId, store, history]);

  useEffect(() => () => {
    const room = store.getState().watchTogether;
    if (room.isInRoom && !room.externalSync) store.dispatch(leaveRoom(room.roomCode));
  }, [store]);
}
