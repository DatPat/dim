import { createContext, useCallback, useContext, useEffect, useMemo, useRef } from "react";
import { useDispatch, useSelector } from "react-redux";
import { useHistory } from "react-router-dom";

import SyncplayClient from "./SyncplayClient";
import { wtAddChatMessage } from "../actions/watchTogether";
import { WT_SET_EXTERNAL_HOST_FILE } from "../actions/types";

const ExternalSyncContext = createContext(null);

export function useExternalSync() {
  return useContext(ExternalSyncContext);
}

export function ExternalSyncProvider({ children }) {
  const dispatch = useDispatch();
  const history = useHistory();
  const externalSync = useSelector((store) => store.watchTogether.externalSync);
  const auth = useSelector((store) => store.auth);
  const clientRef = useRef(null);
  const listenersRef = useRef(new Set());
  const currentFileIdRef = useRef(null);

  // Refs so async/effect callbacks always use the latest values
  // without triggering effect re-runs (which would reconnect).
  const tokenRef = useRef(auth.token);
  useEffect(() => { tokenRef.current = auth.token; }, [auth.token]);
  const historyRef = useRef(history);
  useEffect(() => { historyRef.current = history; }, [history]);

  const onStateUpdate = useCallback((fn) => {
    listenersRef.current.add(fn);
    return () => listenersRef.current.delete(fn);
  }, []);

  const sendChat = useCallback((text) => {
    clientRef.current?.sendChat(text);
  }, []);

  useEffect(() => {
    if (!externalSync) {
      if (clientRef.current) {
        clientRef.current.disconnect();
        clientRef.current = null;
      }
      currentFileIdRef.current = null;
      return;
    }

    const client = new SyncplayClient({
      host: externalSync.host,
      port: externalSync.port,
      room: externalSync.room,
      username: externalSync.username,
      password: externalSync.password,
      // The proxy endpoint requires in-band auth (WebSocket upgrades can't
      // carry the Authorization header).
      token: tokenRef.current,
    });

    client.onStateUpdate = (position, paused, doSeek) => {
      listenersRef.current.forEach((fn) => fn(position, paused, doSeek));
    };

    client.onChat = (username, message) => {
      dispatch(
        wtAddChatMessage({
          user_id: 0,
          username,
          text: message,
          timestamp_ms: Date.now(),
        })
      );
    };

    client.onFileChange = async (fileId, fileName) => {
      if (fileId === currentFileIdRef.current) return;

      currentFileIdRef.current = fileId;

      dispatch({
        type: WT_SET_EXTERNAL_HOST_FILE,
        payload: { fileId, fileName, status: "checking" },
      });

      let fileExists = false;
      try {
        const token = tokenRef.current;
        const res = await fetch(`/api/v1/mediafile/${fileId}`, {
          headers: token ? { authorization: token } : {},
        });
        fileExists = res.ok;
      } catch {
        // file check failed
      }

      if (fileExists) {
        dispatch({
          type: WT_SET_EXTERNAL_HOST_FILE,
          payload: { fileId, fileName, status: "found" },
        });

        const currentPath = window.location.pathname;
        if (currentPath !== `/play/${fileId}`) {
          historyRef.current.push(`/play/${fileId}`);
        }

        client.setReady(true);
      } else {
        dispatch({
          type: WT_SET_EXTERNAL_HOST_FILE,
          payload: { fileId, fileName, status: "missing" },
        });
      }
    };

    client.connect();
    clientRef.current = client;

    return () => {
      client.disconnect();
      clientRef.current = null;
      currentFileIdRef.current = null;
    };
  // Only reconnect when externalSync changes (connect/disconnect).
  // dispatch is stable. history is accessed via ref.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [externalSync]);

  const value = useMemo(
    () => ({ clientRef, onStateUpdate, sendChat }),
    [onStateUpdate, sendChat]
  );

  return (
    <ExternalSyncContext.Provider value={value}>
      {children}
    </ExternalSyncContext.Provider>
  );
}
