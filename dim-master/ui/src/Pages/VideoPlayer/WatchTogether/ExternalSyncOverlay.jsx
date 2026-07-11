import { useCallback, useEffect, useRef, useState } from "react";
import { useDispatch, useSelector } from "react-redux";

import { useExternalSync } from "../../../Controllers/ExternalSyncContext";
import { wtToggleChat } from "../../../actions/watchTogether";
import { useMouseProximity } from "../../../hooks/useMouseProximity";
import ChatMessage from "./ChatMessage";

import "./WatchTogether.scss";

function ExternalSyncOverlay() {
  const dispatch = useDispatch();
  const { sendChat } = useExternalSync();
  const { chatOpen, chatMessages, externalSync, externalHostFile, externalIsGuest } =
    useSelector((store) => store.watchTogether);

  const [text, setText] = useState("");
  const messagesEndRef = useRef(null);
  const overlayRef = useRef(null);
  const isNear = useMouseProximity(overlayRef);
  const visible = isNear || chatOpen;

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [chatMessages]);

  const handleToggleChat = useCallback(() => {
    dispatch(wtToggleChat());
  }, [dispatch]);

  const handleSend = useCallback(
    (e) => {
      e.preventDefault();
      const trimmed = text.trim();
      if (!trimmed) return;
      sendChat(trimmed);
      setText("");
    },
    [text, sendChat]
  );

  const handleLeave = useCallback(() => {
    dispatch({ type: "WT_LEAVE_ROOM" });
  }, [dispatch]);

  return (
    <div
      className={`wtOverlay ${visible ? "visible" : "hidden"}`}
      ref={overlayRef}
    >
      <div className="wtRoomInfo">
        <div className="wtRoomCode">
          <span className="wtLabel">Syncplay</span>
          <span className="wtCode">
            {externalSync?.host}:{externalSync?.port} / {externalSync?.room}
          </span>
        </div>
        {externalIsGuest && (
          <span className="wtHostBadge" style={{ background: "rgba(100,200,100,0.3)" }}>
            Guest
          </span>
        )}
        {!externalIsGuest && (
          <span className="wtHostBadge">Host</span>
        )}
        <div className="wtActions">
          <button className="wtChatBtn" onClick={handleToggleChat}>
            Chat
          </button>
          <button className="wtLeaveBtn" onClick={handleLeave}>
            Disconnect
          </button>
        </div>
      </div>

      {externalHostFile && externalHostFile.status === "checking" && (
        <div className="wtFileStatus">
          Checking for host file: {externalHostFile.fileName}...
        </div>
      )}
      {externalHostFile && externalHostFile.status === "missing" && (
        <div className="wtFileStatus wtFileMissing">
          Missing file: {externalHostFile.fileName}
        </div>
      )}
      {externalHostFile && externalHostFile.status === "found" && externalIsGuest && (
        <div className="wtFileStatus wtFileReady">
          Ready — following host playback
        </div>
      )}

      {chatOpen && (
        <div className="wtChatPanel">
          <div className="wtChatMessages">
            {chatMessages.map((msg, i) => (
              <ChatMessage key={i} message={msg} />
            ))}
            <div ref={messagesEndRef} />
          </div>
          <form className="wtChatInput" onSubmit={handleSend}>
            <input
              type="text"
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="Type a message..."
              maxLength={500}
            />
            <button type="submit">Send</button>
          </form>
        </div>
      )}
    </div>
  );
}

export default ExternalSyncOverlay;
