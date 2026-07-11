function ChatMessage({ message }) {
  const time = new Date(message.timestamp_ms);
  const timeStr = `${time.getHours().toString().padStart(2, "0")}:${time.getMinutes().toString().padStart(2, "0")}`;

  return (
    <div className="wtChatMessage">
      <span className="wtChatTime">{timeStr}</span>
      <span className="wtChatUser">{message.username}</span>
      <span className="wtChatText">{message.text}</span>
    </div>
  );
}

export default ChatMessage;
