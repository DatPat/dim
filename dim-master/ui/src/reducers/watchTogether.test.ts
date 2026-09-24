import reducer from "./watchTogether";
import { WT_SET_ROOM, WT_UPDATE_PARTICIPANTS, WT_SET_READY } from "../actions/types";

const participants = [
  { user_id: -3, username: "pat", picture: null, is_host: false, is_buffering: false, is_ready: false, client_type: "syncplay" },
  { user_id: 5, username: "pat (2)", picture: null, is_host: true, is_buffering: false, is_ready: false, client_type: "dim" },
];

test("local user is matched by id even when the server renamed them", () => {
  let state = reducer(undefined, { type: WT_SET_ROOM, payload: { code: "ABC", participants } });
  state = reducer(state, { type: WT_UPDATE_PARTICIPANTS,
    payload: { participants, currentUsername: "pat", currentUserId: 5 } });
  expect(state.isHost).toBe(true);
  state = reducer(state, { type: WT_SET_READY,
    payload: { user_id: -3, is_ready: true, currentUsername: "pat", currentUserId: 5 } });
  expect(state.isReady).toBe(false);
  state = reducer(state, { type: WT_SET_READY,
    payload: { user_id: 5, is_ready: true, currentUsername: "pat", currentUserId: 5 } });
  expect(state.isReady).toBe(true);
});
