import { useCallback, useContext, useRef, useState } from "react";
import { useDispatch, useSelector } from "react-redux";

import { WebSocketContext } from "../../../Components/WS";
import { leaveRoom, wtToggleChat } from "../../../actions/watchTogether";
import { useMouseProximity } from "../../../hooks/useMouseProximity";

import ParticipantList from "./ParticipantList";
import ChatPanel from "./ChatPanel";

import "./WatchTogether.scss";

function RoomOverlay() {
  const dispatch = useDispatch();
  const ws = useContext(WebSocketContext);
  const { roomCode, isHost, chatOpen, controlMode, isReady } = useSelector(
    (store) => store.watchTogether
  );
  const [copied, setCopied] = useState(false);
  const overlayRef = useRef(null);
  const isNear = useMouseProximity(overlayRef);
  const visible = isNear || chatOpen;

  const copyCode = useCallback(() => {
    navigator.clipboard.writeText(roomCode).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  }, [roomCode]);

  const handleLeave = useCallback(() => {
    dispatch(leaveRoom(roomCode));
  }, [dispatch, roomCode]);

  const handleToggleChat = useCallback(() => {
    dispatch(wtToggleChat());
  }, [dispatch]);

  const handleToggleReady = useCallback(() => {
    if (!ws || !roomCode) return;
    ws.send(
      JSON.stringify({
        type: "watch_together_ready",
        room_code: roomCode,
        is_ready: !isReady,
      })
    );
  }, [ws, roomCode, isReady]);

  const modeBadge =
    controlMode === "egalitarian" ? "Syncplay" : isHost ? "Host" : null;

  return (
    <div
      className={`wtOverlay ${visible ? "visible" : "hidden"}`}
      ref={overlayRef}
    >
      <div className="wtRoomInfo">
        <div className="wtRoomCode" onClick={copyCode} title="Click to copy">
          <span className="wtLabel">Room</span>
          <span className="wtCode">{roomCode}</span>
          {copied && <span className="wtCopied">Copied!</span>}
        </div>
        {modeBadge && <span className="wtHostBadge">{modeBadge}</span>}
        <ParticipantList />
        <div className="wtActions">
          <button
            className={`wtReadyBtn ${isReady ? "ready" : ""}`}
            onClick={handleToggleReady}
          >
            {isReady ? "Ready" : "Not Ready"}
          </button>
          <button className="wtChatBtn" onClick={handleToggleChat}>
            Chat
          </button>
          <button className="wtLeaveBtn" onClick={handleLeave}>
            Leave
          </button>
        </div>
      </div>
      {chatOpen && <ChatPanel />}
    </div>
  );
}

export default RoomOverlay;
