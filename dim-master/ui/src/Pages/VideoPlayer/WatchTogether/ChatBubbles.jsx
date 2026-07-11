import { useEffect, useRef, useState } from "react";
import { useSelector } from "react-redux";

import "./WatchTogether.scss";

const BUBBLE_LIFETIME_MS = 6000;

function ChatBubbles() {
  const { chatMessages, chatOpen, isInRoom, externalSync } = useSelector(
    (store) => store.watchTogether
  );
  const seenCountRef = useRef(null);
  const [bubbles, setBubbles] = useState([]);

  useEffect(() => {
    // Baseline on first run so we don't replay history that was already in
    // the store when the component mounted.
    if (seenCountRef.current === null) {
      seenCountRef.current = chatMessages.length;
      return;
    }

    if (chatMessages.length <= seenCountRef.current) {
      // List shrunk (e.g., room reset) — re-baseline.
      seenCountRef.current = chatMessages.length;
      return;
    }

    const fresh = chatMessages.slice(seenCountRef.current);
    seenCountRef.current = chatMessages.length;

    const additions = fresh.map((message, i) => ({
      id: `${message.timestamp_ms}-${i}-${Math.random().toString(36).slice(2, 7)}`,
      message,
    }));

    setBubbles((prev) => [...prev, ...additions]);

    additions.forEach((bubble) => {
      setTimeout(() => {
        setBubbles((prev) => prev.filter((b) => b.id !== bubble.id));
      }, BUBBLE_LIFETIME_MS);
    });
  }, [chatMessages]);

  // Hide entirely if the chat panel is open (it shows the same content) or
  // if we're not in any sync session.
  if (chatOpen) return null;
  if (!isInRoom && !externalSync) return null;
  if (bubbles.length === 0) return null;

  return (
    <div className="wtChatBubbles">
      {bubbles.map((bubble) => (
        <div key={bubble.id} className="wtChatBubble">
          <span className="wtChatBubbleUser">{bubble.message.username}</span>
          <span className="wtChatBubbleText">{bubble.message.text}</span>
        </div>
      ))}
    </div>
  );
}

export default ChatBubbles;
