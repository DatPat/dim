import { useCallback, useState } from "react";
import { useDispatch, useSelector } from "react-redux";
import Modal from "react-modal";

import Button from "../../Components/Misc/Button";

const WT_SET_ROOM = "WT_SET_ROOM";

function ExternalSyncplayModal({ visible, onClose }) {
  const dispatch = useDispatch();
  const currentUsername = useSelector((store) => store.user?.info?.username || "");

  const [host, setHost] = useState("syncplay.pl");
  const [port, setPort] = useState("8999");
  const [room, setRoom] = useState("");
  const [username, setUsername] = useState(currentUsername);
  const [password, setPassword] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState(null);

  const handleConnect = useCallback(() => {
    if (!host || !port || !room) {
      setError("Host, port, and room are required.");
      return;
    }

    setConnecting(true);
    setError(null);

    // Set up the external sync state in Redux — ExternalSyncController picks it up
    dispatch({
      type: WT_SET_ROOM,
      payload: {
        code: `ext:${room}`,
        isHost: false,
        media_file_id: null,
        media_name: `External: ${room}`,
        playback_state: "paused",
        playback_position: 0,
        server_time_ms: Date.now(),
        participants: [],
        control_mode: "egalitarian",
        externalSync: {
          host,
          port: parseInt(port, 10),
          room,
          username: username || currentUsername || "DimUser",
          password: password || undefined,
        },
      },
    });

    setConnecting(false);
    onClose();
  }, [host, port, room, username, currentUsername, password, dispatch, onClose]);

  const inputStyle = {
    width: "100%",
    padding: "8px 10px",
    background: "rgba(255,255,255,0.1)",
    border: "1px solid rgba(255,255,255,0.2)",
    borderRadius: 6,
    color: "#fff",
    outline: "none",
    fontSize: 14,
    marginBottom: 10,
  };

  const labelStyle = {
    display: "block",
    marginBottom: 4,
    color: "#ccc",
    fontSize: 13,
  };

  return (
    <Modal
      isOpen={visible}
      className="modalBox"
      onRequestClose={onClose}
      overlayClassName="popupOverlay"
    >
      <div className="modalSelectMediaFile">
        <div className="header">
          <h3>Connect to Syncplay Server</h3>
          <p className="desc">
            Join an external Syncplay server to sync playback with desktop
            clients.
          </p>
        </div>
        <div className="separator" />
        <div style={{ padding: "10px 20px" }}>
          <div style={{ display: "flex", gap: 8 }}>
            <div style={{ flex: 3 }}>
              <label style={labelStyle}>Server Host</label>
              <input
                type="text"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="syncplay.pl"
                style={inputStyle}
                autoFocus
              />
            </div>
            <div style={{ flex: 1 }}>
              <label style={labelStyle}>Port</label>
              <input
                type="text"
                value={port}
                onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
                placeholder="8999"
                style={inputStyle}
              />
            </div>
          </div>
          <label style={labelStyle}>Room Name</label>
          <input
            type="text"
            value={room}
            onChange={(e) => setRoom(e.target.value)}
            placeholder="my-room"
            style={inputStyle}
          />
          <label style={labelStyle}>Username</label>
          <input
            type="text"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            placeholder={currentUsername || "DimUser"}
            style={inputStyle}
          />
          <label style={labelStyle}>Password (optional)</label>
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="Leave blank if none"
            style={inputStyle}
          />
        </div>
        {error && (
          <p style={{ color: "#e74c3c", padding: "0 20px" }}>{error}</p>
        )}
        <div className="options" style={{ display: "flex", gap: 8 }}>
          <Button onClick={onClose}>Cancel</Button>
          <Button type="icon" onClick={handleConnect} disabled={connecting}>
            <p>{connecting ? "Connecting..." : "Connect"}</p>
          </Button>
        </div>
      </div>
    </Modal>
  );
}

export default ExternalSyncplayModal;
