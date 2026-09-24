import {
  WT_SET_ROOM,
  WT_LEAVE_ROOM,
  WT_SYNC_PLAYBACK,
  WT_UPDATE_PARTICIPANTS,
  WT_ADD_CHAT_MESSAGE,
  WT_ROOM_DESTROYED,
  WT_SET_BUFFERING,
  WT_TOGGLE_CHAT,
  WT_SET_ERROR,
  WT_SET_READY,
  WT_SET_CONTROL_MODE,
  WT_MODE_CHANGED,
  WT_EXPECT_EPISODE,
} from "./types";
import { stampRoom } from "../Pages/VideoPlayer/WatchTogether/timing";
import { addNotification } from "../slices/notifications";

// Preserve server mutation order too: a slow reconnect request must complete
// before a subsequent Leave, otherwise it could recreate a ghost membership.
const roomRequests = new WeakMap();
function requestRoom(getState, url, options) {
  const previous = roomRequests.get(getState) || Promise.resolve();
  const request = previous.then(async () => {
    const startedAt = performance.now();
    const res = await fetch(url, options);
    return { res, startedAt };
  });
  roomRequests.set(getState, request.then(() => {}, () => {}));
  return request;
}

function getAuthHeaders(getState) {
  return {
    "Content-Type": "application/json",
    Authorization: getState().auth.token,
  };
}

export const createRoom = (mediaFileId, mediaId, mediaName, controlMode, password) => async (dispatch, getState) => {
  try {
    const { res, startedAt } = await requestRoom(getState, "/api/v1/watch-together/rooms", {
      method: "POST",
      headers: getAuthHeaders(getState),
      body: JSON.stringify({
        media_file_id: mediaFileId,
        media_id: mediaId,
        media_name: mediaName,
        control_mode: controlMode || undefined,
        password: password || undefined,
      }),
    });

    if (!res.ok) throw new Error("Failed to create room");

    const room = stampRoom(await res.json(), startedAt);
    dispatch({
      type: WT_SET_ROOM,
      payload: { ...room, isHost: true, roomPassword: password, externalSync: null },
    });

    return room;
  } catch (e) {
    dispatch({ type: WT_SET_ERROR, payload: e.message });
    return null;
  }
};

export const joinRoom = (code, password, reconnect = false) => async (dispatch, getState) => {
  const previousRoom = getState().watchTogether;
  const stillCurrent = () => !reconnect || getState().watchTogether === previousRoom || (
    getState().watchTogether.isInRoom &&
    getState().watchTogether.roomCode === code &&
    getState().watchTogether.sessionVersion === previousRoom.sessionVersion
  );
  try {
    const body = password ? { password } : undefined;
    const { res, startedAt } = await requestRoom(getState, `/api/v1/watch-together/rooms/${code}/join`, {
      method: "POST",
      headers: getAuthHeaders(getState),
      body: body ? JSON.stringify(body) : undefined,
    });

    if (!res.ok) {
      const msg = res.status === 404 ? "Room not found"
        : res.status === 403 ? "Invalid password"
        : "Failed to join room";
      throw new Error(msg);
    }

    const room = stampRoom(await res.json(), startedAt);
    if (!stillCurrent()) return null;
    const localUser = getState().user?.info;
    const participant = room.participants.find((p) => localUser?.id != null
      ? p.user_id === localUser.id : p.username === localUser?.username);
    dispatch({
      type: WT_SET_ROOM,
      payload: { ...room, isHost: participant?.is_host || false,
        isReady: participant?.is_ready || false, roomPassword: password, externalSync: null },
    });

    return room;
  } catch (e) {
    if (!stillCurrent()) return null;
    if (reconnect) {
      dispatch({ type: WT_LEAVE_ROOM });
      dispatch(addNotification({ msg: `Could not restore Watch Together: ${e.message}` }));
    }
    dispatch({ type: WT_SET_ERROR, payload: e.message });
    return null;
  }
};

export const leaveRoom = (code) => async (dispatch, getState) => {
  // Clear local membership before awaiting the network. A delayed leave must
  // never erase a newly joined room or keep controls broadcasting during exit.
  if (getState().watchTogether.roomCode === code) dispatch({ type: WT_LEAVE_ROOM });
  try {
    await requestRoom(getState, `/api/v1/watch-together/rooms/${code}/leave`, {
      method: "POST",
      headers: getAuthHeaders(getState),
    });
  } catch (_e) {
    // Best effort
  }
};

export const reconnectRoom = () => (dispatch, getState) => {
  const room = getState().watchTogether;
  if (!room.isInRoom || room.externalSync) return;
  return dispatch(joinRoom(room.roomCode, room.roomPassword, true));
};

export const transferHost = (code, toUserId) => async (_dispatch, getState) => {
  await fetch(`/api/v1/watch-together/rooms/${code}/transfer-host`, {
    method: "POST",
    headers: getAuthHeaders(getState),
    body: JSON.stringify({ to_user_id: toUserId }),
  });
};

export const changeRoomMedia = (mediaFileId, expectedMediaFileId) => async (dispatch, getState) => {
  const room = getState().watchTogether;
  if (!room.isInRoom || room.externalSync || !room.isHost ||
      String(room.mediaFileId) !== String(expectedMediaFileId)) return;
  // Set before the request: the WebSocket room update can navigate to the
  // new file before this request resolves.
  dispatch({ type: WT_EXPECT_EPISODE, payload: Number(mediaFileId) });
  try {
    const { res, startedAt } = await requestRoom(getState,
      `/api/v1/watch-together/rooms/${room.roomCode}/media`, {
        method: "POST", headers: getAuthHeaders(getState),
        body: JSON.stringify({ media_file_id: Number(mediaFileId),
          expected_media_file_id: Number(expectedMediaFileId) }),
      });
    if (!res.ok) throw new Error("Could not advance the Watch Together episode");
    const updated = stampRoom(await res.json(), startedAt);
    const current = getState().watchTogether;
    // The WebSocket update may already have arrived, or the user may have left.
    if (current.isInRoom && current.roomCode === room.roomCode &&
        current.sessionVersion === room.sessionVersion && current.mediaFileId === room.mediaFileId) {
      const info = getState().user?.info;
      dispatch(wtUpdateParticipants({ ...updated,
        currentUsername: info?.username, currentUserId: info?.id }));
    }
  } catch (e) {
    dispatch({ type: WT_EXPECT_EPISODE, payload: null });
    dispatch(addNotification({ msg: e.message }));
  }
};

export const wtSyncPlayback = (payload) => ({
  type: WT_SYNC_PLAYBACK,
  payload: { client_received_at: performance.now(), ...payload },
});

export const wtUpdateParticipants = (payload) => ({
  type: WT_UPDATE_PARTICIPANTS,
  payload: { client_received_at: performance.now(), ...payload },
});

export const wtAddChatMessage = (payload) => ({
  type: WT_ADD_CHAT_MESSAGE,
  payload,
});

export const wtRoomDestroyed = (payload) => ({
  type: WT_ROOM_DESTROYED,
  payload,
});

export const wtSetBuffering = (payload) => ({
  type: WT_SET_BUFFERING,
  payload,
});

export const wtToggleChat = () => ({
  type: WT_TOGGLE_CHAT,
});

export const wtSetError = (message) => ({
  type: WT_SET_ERROR,
  payload: message,
});

export const wtSetReady = (payload) => ({
  type: WT_SET_READY,
  payload,
});

export const wtSetControlMode = (mode) => ({
  type: WT_SET_CONTROL_MODE,
  payload: mode,
});

export const wtModeChanged = (payload) => ({
  type: WT_MODE_CHANGED,
  payload,
});
