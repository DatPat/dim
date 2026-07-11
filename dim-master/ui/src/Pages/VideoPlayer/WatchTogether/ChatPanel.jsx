import { useCallback, useContext, useEffect, useRef, useState } from "react";
import { useSelector } from "react-redux";

import { WebSocketContext } from "../../../Components/WS";
import ChatMessage from "./ChatMessage";

function ChatPanel() {
  const ws = useContext(WebSocketContext);
  const { roomCode, chatMessages } = useSelector(
    (store) => store.watchTogether
  );
  const [text, setText] = useState("");
  const messagesEndRef = useRef(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [chatMessages]);

  const sendMessage = useCallback(
    (e) => {
      e.preventDefault();
      const trimmed = text.trim();
      if (!trimmed || !ws || !roomCode) return;

      ws.send(
        JSON.stringify({
          type: "watch_together_chat",
          room_code: roomCode,
          text: trimmed,
        })
      );

      setText("");
    },
    [text, ws, roomCode]
  );

  return (
    <div className="wtChatPanel">
      <div className="wtChatMessages">
        {chatMessages.map((msg, i) => (
          <ChatMessage key={i} message={msg} />
        ))}
        <div ref={messagesEndRef} />
      </div>
      <form className="wtChatInput" onSubmit={sendMessage}>
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
  );
}

export default ChatPanel;
