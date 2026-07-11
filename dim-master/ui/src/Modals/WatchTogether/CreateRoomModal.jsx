import { useCallback, useState } from "react";
import { useDispatch, useSelector } from "react-redux";
import { useHistory } from "react-router-dom";
import Modal from "react-modal";

import { createRoom } from "../../actions/watchTogether";
import Button from "../../Components/Misc/Button";

function CreateRoomModal({ visible, onClose, mediaFileId, mediaId, mediaName }) {
  const dispatch = useDispatch();
  const history = useHistory();
  const [creating, setCreating] = useState(false);
  const [controlMode, setControlMode] = useState("host_only");
  const [password, setPassword] = useState("");
  const { error } = useSelector((store) => store.watchTogether);

  const handleCreate = useCallback(async () => {
    setCreating(true);
    const room = await dispatch(
      createRoom(
        mediaFileId,
        mediaId,
        mediaName,
        controlMode,
        password || undefined
      )
    );
    setCreating(false);

    if (room) {
      onClose();
      history.push(`/play/${mediaFileId}?room=${room.code}`);
    }
  }, [dispatch, mediaFileId, mediaId, mediaName, controlMode, password, onClose, history]);

  return (
    <Modal
      isOpen={visible}
      className="modalBox"
      onRequestClose={onClose}
      overlayClassName="popupOverlay"
    >
      <div className="modalSelectMediaFile">
        <div className="header">
          <h3>Watch Together</h3>
          <p className="desc">
            Create a room to watch "{mediaName}" together with friends.
          </p>
        </div>
        <div className="separator" />
        <div style={{ padding: "10px 20px" }}>
          <label style={{ display: "block", marginBottom: 8, color: "#ccc", fontSize: 13 }}>
            Control Mode
          </label>
          <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
            <button
              onClick={() => setControlMode("host_only")}
              style={{
                flex: 1,
                padding: "8px 12px",
                background: controlMode === "host_only" ? "rgba(52, 152, 219, 0.3)" : "rgba(255,255,255,0.05)",
                border: controlMode === "host_only" ? "1px solid #3498db" : "1px solid rgba(255,255,255,0.1)",
                borderRadius: 6,
                color: "#fff",
                cursor: "pointer",
                fontSize: 13,
              }}
            >
              Host Only
            </button>
            <button
              onClick={() => setControlMode("egalitarian")}
              style={{
                flex: 1,
                padding: "8px 12px",
                background: controlMode === "egalitarian" ? "rgba(46, 204, 113, 0.3)" : "rgba(255,255,255,0.05)",
                border: controlMode === "egalitarian" ? "1px solid #2ecc71" : "1px solid rgba(255,255,255,0.1)",
                borderRadius: 6,
                color: "#fff",
                cursor: "pointer",
                fontSize: 13,
              }}
            >
              Syncplay Mode
            </button>
          </div>
          <label style={{ display: "block", marginBottom: 4, color: "#ccc", fontSize: 13 }}>
            Password (optional)
          </label>
          <input
            type="text"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="Leave blank for open room"
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
          <Button type="icon" onClick={handleCreate} disabled={creating}>
            <p>{creating ? "Creating..." : "Create Room"}</p>
          </Button>
        </div>
      </div>
    </Modal>
  );
}

export default CreateRoomModal;
