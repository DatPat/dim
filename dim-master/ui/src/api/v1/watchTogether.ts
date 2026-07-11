import v1 from "./index";

export interface WtParticipant {
  user_id: number;
  username: string;
  picture: number | null;
  is_host: boolean;
  is_buffering: boolean;
  is_ready: boolean;
  client_type: string;
}

export interface WtRoomInfo {
  code: string;
  media_file_id: number | null;
  media_id: number | null;
  media_name: string;
  host_user_id: number;
  playback_state: string;
  playback_position: number;
  server_time_ms: number;
  participants: WtParticipant[];
  control_mode: string;
}

export const watchTogetherApi = v1.injectEndpoints({
  endpoints: (build) => ({
    listRooms: build.query<WtRoomInfo[], void>({
      query: () => "watch-together/rooms",
    }),
  }),
});

export const { useListRoomsQuery } = watchTogetherApi;
