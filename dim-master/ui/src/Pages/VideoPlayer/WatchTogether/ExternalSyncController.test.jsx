import React from "react";
import { render, act } from "@testing-library/react";
import { Provider } from "react-redux";
import { configureStore } from "@reduxjs/toolkit";
import { VideoPlayerContext } from "../Context";
import { useExternalSync } from "../../../Controllers/ExternalSyncContext";
import ExternalSyncController from "./ExternalSyncController";

jest.mock("../../../Controllers/ExternalSyncContext", () => ({ useExternalSync: jest.fn() }));
jest.mock("../../../api/v1/media", () => ({ useGetMediaFilesQuery: () => ({}) }));
jest.mock("react-router-dom", () => ({ useParams: () => ({ fileID: "1" }) }));

test("external guests apply explicit seeks and paused snapshots at zero", () => {
  let onState;
  const clientRef = { current: {} };
  useExternalSync.mockReturnValue({ clientRef, onStateUpdate: cb => { onState = cb; return () => {}; } });
  const store = configureStore({ reducer: {
    watchTogether: () => ({ externalSync: {}, externalIsGuest: true }), video: () => ({}),
  } });
  const video = Object.assign(new EventTarget(), { currentTime: 100, paused: false,
    play: jest.fn(() => Promise.resolve()), pause: jest.fn(),
  });
  const player = { seek: jest.fn() };
  render(<Provider store={store}><VideoPlayerContext.Provider value={{ player, videoRef: { current: video } }}>
    <ExternalSyncController />
  </VideoPlayerContext.Provider></Provider>);
  act(() => onState(0, true, true));
  expect(player.seek).toHaveBeenCalledWith(0);
  expect(video.pause).toHaveBeenCalled();
  player.seek.mockClear(); video.pause.mockClear();
  act(() => onState(0, true, false));
  expect(player.seek).toHaveBeenCalledWith(0);
  expect(video.pause).toHaveBeenCalled();
});

test("host wires native events once the client connects after mount", () => {
  // Connecting from inside the player mounts this controller in the same
  // commit that creates the client — our effects run before it exists.
  const onStateUpdate = () => () => {};
  useExternalSync.mockReturnValue({ clientRef: { current: null }, client: null, onStateUpdate });
  const store = configureStore({ reducer: {
    watchTogether: () => ({ externalSync: {}, externalIsGuest: false }), video: () => ({}),
  } });
  const video = Object.assign(new EventTarget(), { currentTime: 42, paused: false });
  const tree = () => <Provider store={store}><VideoPlayerContext.Provider value={{ player: {}, videoRef: { current: video } }}>
    <ExternalSyncController />
  </VideoPlayerContext.Provider></Provider>;
  const { rerender } = render(tree());
  const client = { reportState: jest.fn(), setFile: jest.fn() };
  useExternalSync.mockReturnValue({ clientRef: { current: client }, client, onStateUpdate });
  rerender(tree());
  act(() => { video.dispatchEvent(new Event("seeked")); });
  expect(client.reportState).toHaveBeenCalledWith(42, false, true);
});
