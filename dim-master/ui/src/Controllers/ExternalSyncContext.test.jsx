import React from "react";
import { render, act } from "@testing-library/react";
import { Provider } from "react-redux";
import { configureStore } from "@reduxjs/toolkit";
import { Router } from "react-router-dom";
import { createMemoryHistory } from "history";
import watchTogether from "../reducers/watchTogether";
import { WT_SET_ROOM, WT_LEAVE_ROOM } from "../actions/types";
import SyncplayClient from "./SyncplayClient";
import { ExternalSyncProvider } from "./ExternalSyncContext";

jest.mock("./SyncplayClient", () => jest.fn());

function setup() {
  const store = configureStore({ reducer: { watchTogether, auth: () => ({ token: "test" }) } });
  store.dispatch({ type: WT_SET_ROOM, payload: { code: "ext:test",
    externalSync: { host: "test", port: 8999, username: "guest", room: "test" },
  } });
  const history = createMemoryHistory();
  const view = render(<Provider store={store}><Router history={history}>
    <ExternalSyncProvider><span /></ExternalSyncProvider>
  </Router></Provider>);
  return { ...view, store, history, client: SyncplayClient.mock.results.at(-1).value };
}
beforeEach(() => {
  global.fetch = jest.fn();
  SyncplayClient.mockImplementation(() => ({
    connect: jest.fn(), disconnect: jest.fn(), setReady: jest.fn(),
  }));
});

test("older file lookup cannot replace the newest host episode", async () => {
  const pending = {};
  fetch.mockImplementation(url => new Promise(resolve => { pending[url] = resolve; }));
  const { client, store, history } = setup();
  let a, b;
  act(() => { a = client.onFileChange(1, "first"); b = client.onFileChange(2, "second"); });
  expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
  await act(async () => { pending["/api/v1/mediafile/2"]({ ok: true }); await b; });
  await act(async () => { pending["/api/v1/mediafile/1"]({ ok: true }); await a; });
  expect(history.location.pathname).toBe("/play/2");
  expect(store.getState().watchTogether.externalHostFile.fileId).toBe(2);
});

test("disconnect invalidates pending file navigation even if fetch resolves", async () => {
  let resolve;
  fetch.mockImplementation(() => new Promise(r => { resolve = r; }));
  const { client, store, history } = setup();
  let pending;
  act(() => { pending = client.onFileChange(1, "first"); store.dispatch({ type: WT_LEAVE_ROOM }); });
  await act(async () => { resolve({ ok: true }); await pending; });
  expect(history.location.pathname).toBe("/");
  expect(store.getState().watchTogether.externalHostFile).toBeNull();
  expect(client.setReady).not.toHaveBeenCalled();
});
