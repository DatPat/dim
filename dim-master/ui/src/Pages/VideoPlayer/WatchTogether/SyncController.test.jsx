import React from "react";
import { render, act } from "@testing-library/react";
import { Provider } from "react-redux";
import { configureStore } from "@reduxjs/toolkit";
import watchTogether from "../../../reducers/watchTogether";
import { WT_SET_ROOM } from "../../../actions/types";
import { WebSocketContext } from "../../../Components/WS";
import { VideoPlayerContext } from "../Context";
import SyncController from "./SyncController";

jest.mock("../../../Components/WS", () => ({ WebSocketContext: require("react").createContext(null) }));

function setup({ host = false, paused = false, canPlay = true } = {}) {
  const store = configureStore({ reducer: {
    watchTogether,
    user: () => ({ info: { id: host ? 1 : 2, username: host ? "host" : "guest" } }),
    auth: () => ({ token: "test" }), video: () => ({ canPlay }),
  } });
  store.dispatch({ type: WT_SET_ROOM, payload: {
    code: "ABC123", isHost: host, media_file_id: 1, playback_position: 100,
    playback_state: paused ? "paused" : "playing", server_time_ms: Date.now() - 60000,
    client_received_at: performance.now(),
  } });
  const video = Object.assign(new EventTarget(), {
    currentTime: 100, paused, ended: false, readyState: 4, playbackRate: 1, seeking: false,
    play: jest.fn(function () { this.paused = false; return Promise.resolve(); }),
    pause: jest.fn(function () { this.paused = true; }),
  });
  const player = { seek: jest.fn(pos => { video.currentTime = pos; }) };
  const ws = Object.assign(new EventTarget(), { send: jest.fn(), readyState: WebSocket.OPEN });
  const view = render(<Provider store={store}><WebSocketContext.Provider value={ws}>
    <VideoPlayerContext.Provider value={{ player, videoRef: { current: video } }}>
      <SyncController />
    </VideoPlayerContext.Provider>
  </WebSocketContext.Provider></Provider>);
  return { ...view, video, player, ws, store };
}

beforeEach(() => {
  jest.useFakeTimers("modern");
  global.fetch = jest.fn(() => Promise.resolve({ ok: true, json: async () => ({
    playback_position: 100, playback_state: "playing", server_time_ms: Date.now() - 60000,
  }) }));
});
afterEach(() => { jest.useRealTimers(); });

test("initial sync and polling ignore server/client clock skew", async () => {
  const { player } = setup();
  await act(async () => {});
  expect(player.seek).toHaveBeenCalledWith(100);
  expect(player.seek).not.toHaveBeenCalledWith(160);
});

test("polling repairs missed pause and play commands", async () => {
  let paused = true;
  fetch.mockImplementation(async () => ({ ok: true, json: async () => ({
    playback_position: 100, playback_state: paused ? "paused" : "playing",
  }) }));
  const { video } = setup();
  await act(async () => {});
  expect(video.paused).toBe(true);
  paused = false;
  await act(async () => { jest.advanceTimersByTime(2000); });
  expect(video.paused).toBe(false);
});

test("a pending poll cannot undo a newer WebSocket command", async () => {
  let resolve;
  fetch.mockImplementation(() => new Promise(r => { resolve = r; }));
  const { video, ws } = setup();
  act(() => { ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify({
    type: "EventWatchTogetherSync", room_code: "ABC123", action: "pause", position: 120,
  }) })); });
  await act(async () => { resolve({ ok: true, json: async () => ({ playback_position: 100, playback_state: "playing" }) }); });
  expect(video.paused).toBe(true);
  expect(video.currentTime).toBe(120);
});

test("unmount cancels polls before they can seek the old player", async () => {
  let resolve;
  fetch.mockImplementation(() => new Promise(r => { resolve = r; }));
  const { player, unmount } = setup();
  unmount(); player.seek.mockClear();
  await act(async () => { resolve({ ok: true, json: async () => ({ playback_position: 900, playback_state: "playing" }) }); });
  expect(player.seek).not.toHaveBeenCalled();
});

test("host reports buffering, resumed playback, and ended state", async () => {
  const { video, ws } = setup({ host: true });
  video.readyState = 2;
  act(() => jest.advanceTimersByTime(4000));
  expect(JSON.parse(ws.send.mock.calls.at(-1)[0])).toMatchObject({ type: "watch_together_state", position: 100, paused: true });
  video.readyState = 4;
  act(() => jest.advanceTimersByTime(1000));
  expect(JSON.parse(ws.send.mock.calls.at(-1)[0]).paused).toBe(false);
  video.ended = true;
  act(() => jest.advanceTimersByTime(1000));
  expect(JSON.parse(ws.send.mock.calls.at(-1)[0]).paused).toBe(true);
});

test("guests and loading hosts do not publish host state", () => {
  const guest = setup();
  act(() => jest.advanceTimersByTime(4000));
  expect(guest.ws.send).not.toHaveBeenCalled();
  guest.unmount();
  const host = setup({ host: true, canPlay: false });
  act(() => jest.advanceTimersByTime(4000));
  expect(host.ws.send).not.toHaveBeenCalled();
});

test("room media updates invalidate old polls and preserve chat and membership", async () => {
  let resolve;
  fetch.mockImplementation(() => new Promise(r => { resolve = r; }));
  const { store, ws, player } = setup();
  player.seek.mockClear();
  act(() => { ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify({
    type: "EventWatchTogetherRoomUpdate", room_code: "ABC123", media_file_id: 2,
    media_name: "Episode 2", playback_state: "playing", playback_position: 0,
    participants: [{ username: "host", is_host: true }, { username: "guest", is_host: false }],
  }) })); });
  await act(async () => { resolve({ ok: true, json: async () => ({
    media_file_id: 1, playback_position: 900, playback_state: "playing",
  }) }); });
  expect(store.getState().watchTogether).toMatchObject({
    isInRoom: true, roomCode: "ABC123", mediaFileId: 2, mediaName: "Episode 2", syncPosition: 0,
  });
  expect(player.seek).not.toHaveBeenCalled();
});

test("polling recovers a missed episode update without seeking the old episode", async () => {
  fetch.mockResolvedValue({ ok: true, json: async () => ({
    media_file_id: 2, media_name: "Episode 2", playback_position: 0, playback_state: "playing",
    participants: [{ username: "guest", is_host: false }],
  }) });
  const { store, player } = setup();
  player.seek.mockClear();
  await act(async () => {});
  expect(store.getState().watchTogether).toMatchObject({ isInRoom: true, mediaFileId: 2, syncPosition: 0 });
  expect(player.seek).not.toHaveBeenCalled();
});
