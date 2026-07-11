import { useCallback, useContext, useEffect, useRef, useState } from "react";
import { useHistory } from "react-router-dom";

import ImageLoad from "../ImageLoad";
import { WebSocketContext } from "../WS";
import { useAppDispatch } from "../../hooks/store";
import { addNotification } from "../../slices/notifications";
import {
  useGetNotificationsQuery,
  useGetUnreadCountQuery,
  useMarkReadMutation,
  useMarkAllReadMutation,
  notificationApi,
} from "../../api/v1/notifications";

import BellIcon from "../../assets/Icons/Bell";

import "./Index.scss";

function timeAgo(ts: number): string {
  const seconds = Math.floor(Date.now() / 1000 - ts);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function NotificationBell() {
  const dispatch = useAppDispatch();
  const history = useHistory();
  const ws = useContext(WebSocketContext);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const { data: countData, refetch: refetchCount } = useGetUnreadCountQuery();
  const { data: notifications, refetch: refetchList } =
    useGetNotificationsQuery(undefined, { skip: !open });
  const [markRead] = useMarkReadMutation();
  const [markAllRead] = useMarkAllReadMutation();

  const unreadCount = countData?.count ?? 0;

  // Listen for real-time notification events
  useEffect(() => {
    if (!ws) return;

    const handler = (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        if (data.type === "EventNewNotification") {
          dispatch(addNotification({ msg: data.title }));
          refetchCount();
          if (open) refetchList();
        }
      } catch {
        // ignore non-JSON messages
      }
    };

    ws.addEventListener("message", handler);
    return () => ws.removeEventListener("message", handler);
  }, [ws, dispatch, refetchCount, refetchList, open]);

  // Close dropdown when clicking outside
  useEffect(() => {
    if (!open) return;

    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };

    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  const toggle = useCallback(() => {
    setOpen((prev) => !prev);
  }, []);

  const handleMarkAllRead = useCallback(async () => {
    await markAllRead();
    refetchCount();
    refetchList();
  }, [markAllRead, refetchCount, refetchList]);

  const handleDismiss = useCallback(
    async (id: number) => {
      await markRead(id);
      refetchCount();
      refetchList();
    },
    [markRead, refetchCount, refetchList]
  );

  const handleOpenMedia = useCallback(
    (mediaId: number | null) => {
      if (mediaId === null) return;
      setOpen(false);
      history.push(`/media/${mediaId}`);
    },
    [history]
  );

  return (
    <div className="notificationBell" ref={ref}>
      <button className="bellButton" onClick={toggle}>
        <BellIcon />
        {unreadCount > 0 && <span className="badge">{unreadCount}</span>}
      </button>
      {open && (
        <div className="notificationDropdown">
          <div className="dropdownHeader">
            <h4>Notifications</h4>
            {unreadCount > 0 && (
              <button className="markAllRead" onClick={handleMarkAllRead}>
                Mark all read
              </button>
            )}
          </div>
          <div className="dropdownList">
            {(!notifications || notifications.length === 0) && (
              <p className="emptyState">No new notifications</p>
            )}
            {notifications?.map((notif) => (
              <div
                key={notif.id}
                className={`notificationItem${
                  notif.media_id !== null ? " clickable" : ""
                }`}
                onClick={() => handleOpenMedia(notif.media_id)}
              >
                {notif.poster_path && (
                  <div className="notifPoster">
                    <ImageLoad src={notif.poster_path}>
                      {({ imageSrc }) =>
                        imageSrc ? <img src={imageSrc} alt="" /> : <></>
                      }
                    </ImageLoad>
                  </div>
                )}
                <div className="notifContent">
                  <p className="notifTitle">{notif.title}</p>
                  {notif.body && (
                    <p className="notifBody">{notif.body}</p>
                  )}
                  <span className="notifTime">
                    {timeAgo(notif.created_at)}
                  </span>
                </div>
                <button
                  className="notifDismiss"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleDismiss(notif.id);
                  }}
                >
                  &times;
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export default NotificationBell;
