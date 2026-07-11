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
} from "./types";

function getAuthHeaders(getState) {
  return {
    "Content-Type": "application/json",
    Authorization: getState().auth.token,
  };
}

export const createRoom = (mediaFileId, mediaId, mediaName, controlMode, password) => async (dispatch, getState) => {
  try {
    const res = await fetch("/api/v1/watch-together/rooms", {
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

    const room = await res.json();
    dispatch({
      type: WT_SET_ROOM,
      payload: { ...room, isHost: true, externalSync: null },
    });

    return room;
  } catch (e) {
    dispatch({ type: WT_SET_ERROR, payload: e.message });
    return null;
  }
};

export const joinRoom = (code, password) => async (dispatch, getState) => {
  try {
    const body = password ? { password } : undefined;
    const res = await fetch(`/api/v1/watch-together/rooms/${code}/join`, {
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

    const room = await res.json();
    dispatch({
      type: WT_SET_ROOM,
      payload: { ...room, isHost: false, externalSync: null },
    });

    return room;
  } catch (e) {
    dispatch({ type: WT_SET_ERROR, payload: e.message });
    return null;
  }
};

export const leaveRoom = (code) => async (dispatch, getState) => {
  try {
    await fetch(`/api/v1/watch-together/rooms/${code}/leave`, {
      method: "POST",
      headers: getAuthHeaders(getState),
    });
  } catch (_e) {
    // Best effort
  }
  dispatch({ type: WT_LEAVE_ROOM });
};

export const transferHost = (code, toUserId) => async (_dispatch, getState) => {
  await fetch(`/api/v1/watch-together/rooms/${code}/transfer-host`, {
    method: "POST",
    headers: getAuthHeaders(getState),
    body: JSON.stringify({ to_user_id: toUserId }),
  });
};

export const wtSyncPlayback = (payload) => ({
  type: WT_SYNC_PLAYBACK,
  payload,
});

export const wtUpdateParticipants = (payload) => ({
  type: WT_UPDATE_PARTICIPANTS,
  payload,
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
