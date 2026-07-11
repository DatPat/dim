import { useCallback, useRef, useState } from "react";
import { useSelector } from "react-redux";
import { skipToken } from "@reduxjs/toolkit/query/react";

import { useGetMediaQuery } from "../../../../api/v1/media";
import CreateRoomModal from "../../../../Modals/WatchTogether/CreateRoomModal";
import JoinRoomModal from "../../../../Modals/WatchTogether/JoinRoomModal";
import ExternalSyncplayModal from "../../../../Modals/WatchTogether/ExternalSyncplayModal";

/** Simple users/sync SVG icon */
function SyncIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
      <path d="M16 11c1.66 0 2.99-1.34 2.99-3S17.66 5 16 5c-1.66 0-3 1.34-3 3s1.34 3 3 3zm-8 0c1.66 0 2.99-1.34 2.99-3S9.66 5 8 5C6.34 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 1.97 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z" />
    </svg>
  );
}

function WatchTogetherAction() {
  const [menuOpen, setMenuOpen] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [showJoin, setShowJoin] = useState(false);
  const [showExternal, setShowExternal] = useState(false);
  const menuRef = useRef(null);

  const { isInRoom } = useSelector((store) => store.watchTogether);
  const video = useSelector((store) => store.video);
  const mediaId = video.mediaID;
  const { data: media } = useGetMediaQuery(mediaId ? mediaId : skipToken);

  const toggleMenu = useCallback(() => {
    setMenuOpen((prev) => !prev);
  }, []);

  const handleCreate = useCallback(() => {
    setMenuOpen(false);
    setShowCreate(true);
  }, []);

  const handleJoin = useCallback(() => {
    setMenuOpen(false);
    setShowJoin(true);
  }, []);

  const handleExternal = useCallback(() => {
    setMenuOpen(false);
    setShowExternal(true);
  }, []);

  // Don't show when already in a room (RoomOverlay handles that)
  if (isInRoom) return null;

  const mediaFileId = video.fileID || parseInt(window.location.pathname.split("/play/")[1]);
  const mediaName = media?.name || "";

  return (
    <>
      <div style={{ position: "relative" }} ref={menuRef}>
        <button
          onClick={toggleMenu}
          className="watchTogetherBtn"
          title="Watch Together"
          style={{
            background: "none",
            border: "none",
            color: "#fff",
            cursor: "pointer",
            padding: "4px 6px",
            display: "flex",
            alignItems: "center",
            opacity: 0.85,
          }}
        >
          <SyncIcon />
        </button>
        {menuOpen && (
          <div
            className="wtActionMenu"
            style={{
              position: "absolute",
              bottom: "100%",
              right: 0,
              marginBottom: 8,
              background: "rgba(0,0,0,0.9)",
              backdropFilter: "blur(8px)",
              borderRadius: 8,
              padding: "4px 0",
              minWidth: 180,
              zIndex: 200,
            }}
          >
            {mediaFileId && (
              <button onClick={handleCreate} style={menuItemStyle}>
                Create Room
              </button>
            )}
            <button onClick={handleJoin} style={menuItemStyle}>
              Join Room
            </button>
            <button onClick={handleExternal} style={menuItemStyle}>
              External Syncplay
            </button>
          </div>
        )}
      </div>

      {mediaFileId && (
        <CreateRoomModal
          visible={showCreate}
          onClose={() => setShowCreate(false)}
          mediaFileId={mediaFileId}
          mediaId={mediaId}
          mediaName={mediaName}
        />
      )}
      <JoinRoomModal visible={showJoin} onClose={() => setShowJoin(false)} />
      <ExternalSyncplayModal
        visible={showExternal}
        onClose={() => setShowExternal(false)}
      />
    </>
  );
}

const menuItemStyle = {
  display: "block",
  width: "100%",
  padding: "8px 16px",
  background: "none",
  border: "none",
  color: "#fff",
  fontSize: 13,
  textAlign: "left",
  cursor: "pointer",
  whiteSpace: "nowrap",
};

export default WatchTogetherAction;
