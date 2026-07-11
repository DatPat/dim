import { useCallback, useState } from "react";
import { useDispatch, useSelector } from "react-redux";
import { useHistory } from "react-router-dom";
import Modal from "react-modal";

import { joinRoom } from "../../actions/watchTogether";
import Button from "../../Components/Misc/Button";

function JoinRoomModal({ visible, onClose }) {
  const dispatch = useDispatch();
  const history = useHistory();
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [joining, setJoining] = useState(false);
  const { error } = useSelector((store) => store.watchTogether);

  const handleJoin = useCallback(async () => {
    const trimmed = code.trim().toUpperCase();
    if (trimmed.length !== 6) return;

    setJoining(true);
    const room = await dispatch(joinRoom(trimmed, password || undefined));
    setJoining(false);

    if (room) {
      onClose();
      history.push(`/play/${room.media_file_id}?room=${trimmed}`);
    }
  }, [code, password, dispatch, onClose, history]);

  const handleKeyDown = useCallback(
    (e) => {
      if (e.key === "Enter") handleJoin();
    },
    [handleJoin]
  );

  return (
    <Modal
      isOpen={visible}
      className="modalBox"
      onRequestClose={onClose}
      overlayClassName="popupOverlay"
    >
      <div className="modalSelectMediaFile">
        <div className="header">
          <h3>Join Room</h3>
          <p className="desc">Enter the 6-character room code to join.</p>
        </div>
        <div className="separator" />
        <div style={{ padding: "10px 20px" }}>
          <input
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value.toUpperCase().slice(0, 6))}
            onKeyDown={handleKeyDown}
            placeholder="ABC123"
            maxLength={6}
            style={{
              width: "100%",
              padding: "10px",
              fontSize: 18,
              fontFamily: "monospace",
              textAlign: "center",
              letterSpacing: 4,
              background: "rgba(255,255,255,0.1)",
              border: "1px solid rgba(255,255,255,0.2)",
              borderRadius: 6,
              color: "#fff",
              outline: "none",
              marginBottom: 10,
            }}
            autoFocus
          />
          <input
            type="text"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Password (if required)"
            style={{
              width: "100%",
              padding: "8px 10px",
              background: "rgba(255,255,255,0.1)",
              border: "1px solid rgba(255,255,255,0.2)",
              borderRadius: 6,
              color: "#fff",
              outline: "none",
              fontSize: 14,
            }}
          />
        </div>
        {error && (
          <p style={{ color: "#e74c3c", padding: "0 20px" }}>{error}</p>
        )}
        <div className="options" style={{ display: "flex", gap: 8 }}>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            type="icon"
            onClick={handleJoin}
            disabled={joining || code.trim().length !== 6}
          >
            <p>{joining ? "Joining..." : "Join"}</p>
          </Button>
        </div>
      </div>
    </Modal>
  );
}

export default JoinRoomModal;
