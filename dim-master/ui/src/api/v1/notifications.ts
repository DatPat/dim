import v1 from "./index";

export interface ServerNotification {
  id: number;
  user_id: number | null;
  category: string;
  title: string;
  body: string | null;
  media_id: number | null;
  poster_path: string | null;
  created_at: number;
}

export const notificationApi = v1.injectEndpoints({
  endpoints: (build) => ({
    getNotifications: build.query<ServerNotification[], void>({
      query: () => "notifications",
    }),
    getUnreadCount: build.query<{ count: number }, void>({
      query: () => "notifications/unread_count",
    }),
    markRead: build.mutation<void, number>({
      query: (id) => ({ url: `notifications/${id}/read`, method: "POST" }),
    }),
    markAllRead: build.mutation<void, void>({
      query: () => ({ url: "notifications/read_all", method: "POST" }),
    }),
  }),
});

export const {
  useGetNotificationsQuery,
  useGetUnreadCountQuery,
  useMarkReadMutation,
  useMarkAllReadMutation,
} = notificationApi;
