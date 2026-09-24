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
  WT_SET_EXTERNAL_HOST_FILE,
  WT_EXPECT_EPISODE,
} from "../actions/types";

interface WtParticipant {
  user_id: number;
  username: string;
  picture: number | null;
  is_host: boolean;
  is_buffering: boolean;
  is_ready: boolean;
  client_type: string;
}

interface ChatMsg {
  user_id: number;
  username: string;
  text: string;
  timestamp_ms: number;
}

interface WatchTogetherState {
  roomCode: string | null;
  isInRoom: boolean;
  isHost: boolean;
  mediaFileId: number | null;
  mediaName: string | null;
  playbackState: string;
  syncPosition: number;
  lastSyncServerTime: number;
  syncReceivedAt: number | null;
  syncTransitSeconds: number;
  roomPassword: string | null;
  sessionVersion: number;
  participants: WtParticipant[];
  chatMessages: ChatMsg[];
  chatOpen: boolean;
  error: string | null;
  controlMode: string;
  isReady: boolean;
  externalSync: null | {
    host: string;
    port: number;
    room: string;
    username: string;
    password?: string;
  };
  /** Tracks the host's file in an external Syncplay session. */
  externalHostFile: null | {
    fileId: number;
    fileName: string;
    /** 'checking' while API call in-flight, 'found' if file exists, 'missing' if not */
    status: "checking" | "found" | "missing";
  };
  /** True when we are a guest following an external Syncplay host. */
  externalIsGuest: boolean;
  /**
   * File the local host just advanced the room to. The room restarts paused
   * on an episode change, so the host's new player must autoplay this file
   * itself (its report then starts the room for everyone).
   */
  autoplayFileId: number | null;
}

const initialState: WatchTogetherState = {
  roomCode: null,
  isInRoom: false,
  isHost: false,
  mediaFileId: null,
  mediaName: null,
  playbackState: "paused",
  syncPosition: 0,
  lastSyncServerTime: 0,
  syncReceivedAt: null,
  syncTransitSeconds: 0,
  roomPassword: null,
  sessionVersion: 0,
  participants: [],
  chatMessages: [],
  chatOpen: false,
  error: null,
  controlMode: "host_only",
  isReady: false,
  externalSync: null,
  externalHostFile: null,
  externalIsGuest: false,
  autoplayFileId: null,
};

export default function watchTogetherReducer(
  state: WatchTogetherState = initialState,
  action: any
): WatchTogetherState {
  switch (action.type) {
    case WT_SET_ROOM:
      return {
        ...state,
        roomCode: action.payload.code,
        isInRoom: true,
        isHost: action.payload.isHost,
        mediaFileId: action.payload.media_file_id,
        mediaName: action.payload.media_name,
        playbackState: action.payload.playback_state,
        syncPosition: action.payload.playback_position,
        lastSyncServerTime: action.payload.server_time_ms,
        syncReceivedAt: action.payload.client_received_at ?? null,
        syncTransitSeconds: action.payload.transit_seconds || 0,
        roomPassword: action.payload.roomPassword || null,
        sessionVersion: state.sessionVersion + 1,
        isReady: action.payload.isReady || false,
        participants: action.payload.participants || [],
        controlMode: action.payload.control_mode || "host_only",
        externalSync: action.payload.externalSync ?? null,
        // Reset guest state — will be determined fresh by onHostStatus.
        externalHostFile: null,
        externalIsGuest: false,
        autoplayFileId: null,
        error: null,
      };
    case WT_LEAVE_ROOM:
      return { ...initialState, sessionVersion: state.sessionVersion + 1 };
    case WT_SYNC_PLAYBACK:
      return {
        ...state,
        playbackState:
          action.payload.action === "play"
            ? "playing"
            : action.payload.action === "pause"
            ? "paused"
            : state.playbackState,
        syncPosition: action.payload.position,
        lastSyncServerTime: action.payload.server_time_ms,
        syncReceivedAt: action.payload.client_received_at ?? null,
      };
    case WT_UPDATE_PARTICIPANTS:
      return {
        ...state,
        mediaFileId: action.payload.media_file_id ?? state.mediaFileId,
        mediaName: action.payload.media_name ?? state.mediaName,
        participants: action.payload.participants,
        // Match by user id when known: the server renames colliding display
        // names (e.g. "pat (2)"), so a username match can miss ourselves.
        isHost: action.payload.participants.some(
          (p: WtParticipant) =>
            p.is_host &&
            (action.payload.currentUserId != null
              ? p.user_id === action.payload.currentUserId
              : p.username === action.payload.currentUsername)
        ),
        playbackState: action.payload.playback_state || state.playbackState,
        syncPosition:
          action.payload.playback_position ?? state.syncPosition,
        lastSyncServerTime:
          action.payload.server_time_ms || state.lastSyncServerTime,
        syncReceivedAt: action.payload.playback_position != null
          ? action.payload.client_received_at ?? null : state.syncReceivedAt,
      };
    case WT_ADD_CHAT_MESSAGE:
      return {
        ...state,
        chatMessages: [...state.chatMessages, action.payload].slice(-200),
      };
    case WT_ROOM_DESTROYED:
      return {
        ...initialState,
        sessionVersion: state.sessionVersion + 1,
        error:
          action.payload.reason === "host_left"
            ? "The host has left the room."
            : "Room was closed.",
      };
    case WT_SET_BUFFERING:
      return {
        ...state,
        participants: state.participants.map((p) =>
          p.user_id === action.payload.user_id
            ? { ...p, is_buffering: action.payload.is_buffering }
            : p
        ),
      };
    case WT_TOGGLE_CHAT:
      return {
        ...state,
        chatOpen: !state.chatOpen,
      };
    case WT_SET_ERROR:
      return {
        ...state,
        error: action.payload,
      };
    case WT_SET_READY: {
      // The event fires for EVERY participant's toggle — only mirror it into
      // the local `isReady` (which drives the Ready button) when it's about
      // the local user, otherwise someone else's toggle flips our button.
      const isLocalUser = action.payload.currentUserId != null
        ? action.payload.user_id === action.payload.currentUserId
        : action.payload.currentUsername != null &&
        state.participants.some(
          (p) =>
            p.user_id === action.payload.user_id &&
            p.username === action.payload.currentUsername
        );
      return {
        ...state,
        isReady: isLocalUser ? action.payload.is_ready : state.isReady,
        participants: state.participants.map((p) =>
          p.user_id === action.payload.user_id
            ? { ...p, is_ready: action.payload.is_ready }
            : p
        ),
      };
    }
    case WT_SET_CONTROL_MODE:
      return {
        ...state,
        controlMode: action.payload,
      };
    case WT_MODE_CHANGED:
      return {
        ...state,
        controlMode: action.payload.control_mode,
      };
    case WT_EXPECT_EPISODE:
      return {
        ...state,
        autoplayFileId: action.payload,
      };
    case WT_SET_EXTERNAL_HOST_FILE:
      return {
        ...state,
        externalHostFile: action.payload,
        // Guest as soon as we receive any host file info (even "checking").
        // Only null payload (reset) clears guest status.
        externalIsGuest: action.payload != null,
      };
    default:
      return state;
  }
}
