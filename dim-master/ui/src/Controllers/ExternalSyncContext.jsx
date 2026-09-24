import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
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
  // State mirror of clientRef: consumers' effects must re-run once the client
  // exists. Our effect runs AFTER the effects of children that mount in the
  // same commit (e.g. connecting from inside the player), so they would
  // otherwise only ever see a null ref.
  const [client, setClient] = useState(null);
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
        setClient(null);
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

    let active = true;
    let fileRequest = 0;
    let fileAbort = null;
    client.onFileChange = async (fileId, fileName) => {
      if (fileId === currentFileIdRef.current) return;

      currentFileIdRef.current = fileId;
      const request = ++fileRequest;
      fileAbort?.abort();
      fileAbort = new AbortController();

      dispatch({
        type: WT_SET_EXTERNAL_HOST_FILE,
        payload: { fileId, fileName, status: "checking" },
      });

      let fileExists = false;
      try {
        const token = tokenRef.current;
        const res = await fetch(`/api/v1/mediafile/${fileId}`, {
          headers: token ? { authorization: token } : {},
          signal: fileAbort.signal,
        });
        fileExists = res.ok;
      } catch {
        // file check failed
      }

      if (!active || request !== fileRequest) return;
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
        // Allow a later announcement to retry a failed/transient lookup.
        currentFileIdRef.current = null;
        dispatch({
          type: WT_SET_EXTERNAL_HOST_FILE,
          payload: { fileId, fileName, status: "missing" },
        });
      }
    };

    client.connect();
    clientRef.current = client;
    setClient(client);

    return () => {
      active = false;
      fileAbort?.abort();
      client.disconnect();
      clientRef.current = null;
      setClient(null);
      currentFileIdRef.current = null;
    };
  // Only reconnect when externalSync changes (connect/disconnect).
  // dispatch is stable. history is accessed via ref.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [externalSync]);

  const value = useMemo(
    () => ({ clientRef, client, onStateUpdate, sendChat }),
    [client, onStateUpdate, sendChat]
  );

  return (
    <ExternalSyncContext.Provider value={value}>
      {children}
    </ExternalSyncContext.Provider>
  );
}
