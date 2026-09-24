import React from "react";
import { render, act } from "@testing-library/react";
import { Provider } from "react-redux";
import { configureStore } from "@reduxjs/toolkit";
import { Router, Route, useParams } from "react-router-dom";
import { createMemoryHistory } from "history";
import watchTogether from "../../../reducers/watchTogether";
import { WT_SET_ROOM } from "../../../actions/types";
import { leaveRoom, reconnectRoom, wtUpdateParticipants } from "../../../actions/watchTogether";
import useRoomLifecycle, { removeRoomFromUrl } from "./useRoomLifecycle";
import useEpisodeNavigation from "./useEpisodeNavigation";

const roomInfo = { code: "ABC123", media_file_id: 1, playback_state: "paused", playback_position: 100,
  participants: [{ user_id: 1, username: "host", is_host: true, is_ready: true }], control_mode: "host_only" };
function setup({ host = true } = {}) {
  const store = configureStore({ reducer: { watchTogether,
    auth: () => ({ token: "test" }), user: () => ({ info: { id: 1, username: "host" } }),
  } });
  store.dispatch({ type: WT_SET_ROOM, payload: { ...roomInfo, isHost: host, roomPassword: "secret" } });
  const history = createMemoryHistory({ initialEntries: ["/play/1?room=ABC123"] });
  let nextEpisode;
  function Player() {
    const { fileID } = useParams();
    useRoomLifecycle(fileID);
    nextEpisode = useEpisodeNavigation(fileID);
    return null;
  }
  const view = render(<Provider store={store}><Router history={history}>
    <Route path="/play/:fileID"><Player /></Route>
  </Router></Provider>);
  return { ...view, store, history, nextEpisode: (id) => nextEpisode(id) };
}
beforeEach(() => { global.fetch = jest.fn(async () => ({ ok: true, json: async () => roomInfo })); });

test("Leave removes the URL code and sends one leave without rejoining", async () => {
  const { store, history, unmount } = setup();
  await act(async () => { removeRoomFromUrl(history); await store.dispatch(leaveRoom("ABC123")); });
  expect(history.location.search).toBe("");
  expect(store.getState().watchTogether.isInRoom).toBe(false);
  unmount();
  expect(fetch.mock.calls.map(([url]) => url)).toEqual(["/api/v1/watch-together/rooms/ABC123/leave"]);
});

test("unrelated player navigation leaves the old room exactly once", async () => {
  const { history, store } = setup();
  await act(async () => history.replace("/play/2"));
  expect(store.getState().watchTogether.isInRoom).toBe(false);
  expect(fetch.mock.calls.map(([url]) => url)).toEqual(["/api/v1/watch-together/rooms/ABC123/leave"]);
});

test("the host advances the existing room and keeps its password and membership", async () => {
  const { nextEpisode, history, store } = setup();
  fetch.mockResolvedValue({ ok: true, json: async () => ({ ...roomInfo,
    media_file_id: 2, media_name: "Episode 2", playback_position: 0, playback_state: "playing",
  }) });
  await act(async () => { await nextEpisode(2); });
  expect(history.location.pathname + history.location.search).toBe("/play/2?room=ABC123");
  expect(store.getState().watchTogether).toMatchObject({
    isInRoom: true, isHost: true, roomCode: "ABC123", roomPassword: "secret",
    mediaFileId: 2, playbackState: "playing", syncPosition: 0,
  });
  expect(fetch.mock.calls.map(([url]) => url)).toEqual(["/api/v1/watch-together/rooms/ABC123/media"]);
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ media_file_id: 2, expected_media_file_id: 1 });
});

test.each(["host_only", "egalitarian"])("guests wait for the host in %s rooms and follow without leaving", async (mode) => {
  const { nextEpisode, history, store } = setup({ host: false });
  act(() => { store.dispatch({ type: WT_SET_ROOM, payload: { ...roomInfo, isHost: false, control_mode: mode } }); });
  await act(async () => { await nextEpisode(2); });
  expect(history.location.pathname).toBe("/play/1");
  expect(fetch).not.toHaveBeenCalled();
  await act(async () => { store.dispatch(wtUpdateParticipants({ ...roomInfo,
    media_file_id: 2, playback_position: 0, playback_state: "playing", currentUsername: "guest",
  })); });
  expect(history.location.pathname + history.location.search).toBe("/play/2?room=ABC123");
  expect(store.getState().watchTogether.isInRoom).toBe(true);
  expect(fetch).not.toHaveBeenCalled();
});

test("a failed episode request keeps the host in the current room", async () => {
  const { nextEpisode, history, store } = setup();
  fetch.mockResolvedValue({ ok: false, status: 500 });
  await act(async () => { await nextEpisode(2); });
  expect(history.location.pathname).toBe("/play/1");
  expect(store.getState().watchTogether).toMatchObject({ isInRoom: true, mediaFileId: 1 });
  expect(fetch).toHaveBeenCalledTimes(1);
});

test("solo playback still advances directly", async () => {
  const { nextEpisode, store, history } = setup();
  await act(async () => { removeRoomFromUrl(history); await store.dispatch(leaveRoom("ABC123")); });
  fetch.mockClear();
  await act(async () => { await nextEpisode(2); });
  expect(history.location.pathname).toBe("/play/2");
  expect(fetch).not.toHaveBeenCalled();
});

test("repeated ended events send only one transition while the request is pending", async () => {
  const { nextEpisode, history } = setup();
  let resolve;
  fetch.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  let advancing;
  await act(async () => { advancing = nextEpisode(2); await nextEpisode(2); });
  expect(fetch).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve({ ok: true, json: async () => ({ ...roomInfo, media_file_id: 2 }) });
    await advancing;
    await nextEpisode(2);
  });
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(history.location.pathname).toBe("/play/2");
});

test("a delayed episode response cannot bring someone back after Leave", async () => {
  const { nextEpisode, store, history } = setup();
  let resolve;
  fetch.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  let advancing;
  await act(async () => { advancing = nextEpisode(2); });
  let leaving;
  act(() => { removeRoomFromUrl(history); leaving = store.dispatch(leaveRoom("ABC123")); });
  await act(async () => {
    resolve({ ok: true, json: async () => ({ ...roomInfo, media_file_id: 2 }) });
    await advancing; await leaving;
  });
  expect(store.getState().watchTogether.isInRoom).toBe(false);
  expect(history.location.pathname).toBe("/play/1");
});

test("reconnect reuses the password and adopts the actual host and ready status", async () => {
  const { store } = setup();
  fetch.mockResolvedValue({ ok: true, json: async () => ({ ...roomInfo,
    participants: [{ user_id: 1, username: "host", is_host: false, is_ready: true }],
  }) });
  await act(async () => { await store.dispatch(reconnectRoom()); });
  expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ password: "secret" });
  expect(store.getState().watchTogether).toMatchObject({ isInRoom: true, isHost: false, isReady: true });
});

test("expired room on reconnect clears membership without an auto-join loop", async () => {
  const { store } = setup();
  fetch.mockResolvedValue({ ok: false, status: 404 });
  await act(async () => { await store.dispatch(reconnectRoom()); });
  expect(store.getState().watchTogether).toMatchObject({ isInRoom: false, error: "Room not found" });
  expect(fetch).toHaveBeenCalledTimes(1);
});

test("a late reconnect response cannot undo an intentional Leave", async () => {
  const { store, history } = setup();
  let resolve;
  fetch.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  let reconnect;
  await act(async () => { reconnect = store.dispatch(reconnectRoom()); });
  let leaving;
  act(() => { removeRoomFromUrl(history); leaving = store.dispatch(leaveRoom("ABC123")); });
  expect(fetch).toHaveBeenCalledTimes(1);
  await act(async () => { resolve({ ok: true, json: async () => roomInfo }); await reconnect; await leaving; });
  expect(store.getState().watchTogether.isInRoom).toBe(false);
  expect(fetch.mock.calls.map(([url]) => url)).toEqual([
    "/api/v1/watch-together/rooms/ABC123/join", "/api/v1/watch-together/rooms/ABC123/leave",
  ]);
});

test("late leave completion cannot erase a different new room", async () => {
  const { store } = setup();
  let resolve;
  fetch.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  let leaving;
  await act(async () => {
    leaving = store.dispatch(leaveRoom("ABC123"));
    store.dispatch({ type: WT_SET_ROOM, payload: { ...roomInfo, code: "NEW123" } });
  });
  await act(async () => { resolve({ ok: true }); await leaving; });
  expect(store.getState().watchTogether.roomCode).toBe("NEW123");
});
