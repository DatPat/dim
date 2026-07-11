import { useState } from "react";
import { useSelector } from "react-redux";
import { useHistory } from "react-router-dom";

import { useListRoomsQuery } from "../../api/v1/watchTogether";
import JoinRoomModal from "../../Modals/WatchTogether/JoinRoomModal";
import ExternalSyncplayModal from "../../Modals/WatchTogether/ExternalSyncplayModal";

function PeopleIcon() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
      <path d="M16 11c1.66 0 2.99-1.34 2.99-3S17.66 5 16 5c-1.66 0-3 1.34-3 3s1.34 3 3 3zm-8 0c1.66 0 2.99-1.34 2.99-3S9.66 5 8 5C6.34 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 1.97 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z" />
    </svg>
  );
}

function PlayIcon() {
  return (
    <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
      <path d="M8 5v14l11-7z" />
    </svg>
  );
}

function PauseIcon() {
  return (
    <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
      <path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z" />
    </svg>
  );
}

function WatchTogether() {
  const history = useHistory();
  const { isInRoom, roomCode, externalSync } = useSelector(
    (store) => store.watchTogether
  );

  const auth = useSelector((store) => store.auth);
  const { data: rooms = [] } = useListRoomsQuery(undefined, {
    pollingInterval: 10000,
    skip: !auth.token,
  });

  const [joinOpen, setJoinOpen] = useState(false);
  const [syncplayOpen, setSyncplayOpen] = useState(false);

  const handleJoinRoom = (room) => {
    if (room.media_file_id) {
      history.push(`/play/${room.media_file_id}?room=${room.code}`);
    }
  };

  const activeRoomCode = externalSync ? null : roomCode;

  return (
    <section className="watchTogether">
      <header>
        <h4>Watch Together</h4>
      </header>

      {rooms.length > 0 && (
        <div className="roomCards">
          {rooms.map((room) => (
            <button
              key={room.code}
              className={`roomCard${
                activeRoomCode === room.code ? " active" : ""
              }`}
              onClick={() => handleJoinRoom(room)}
            >
              <div className="roomInfo">
                <span className="roomName">
                  {room.media_name || room.code}
                </span>
                <span className="roomMeta">
                  <span className="roomState">
                    {room.playback_state === "playing" ? (
                      <PlayIcon />
                    ) : (
                      <PauseIcon />
                    )}
                  </span>
                  {room.participants.length}{" "}
                  {room.participants.length === 1 ? "viewer" : "viewers"}
                  {room.participants.length > 0 && (
                    <span className="roomUsers">
                      {" \u00B7 "}
                      {room.participants.map((p) => p.username).join(", ")}
                    </span>
                  )}
                </span>
              </div>
            </button>
          ))}
        </div>
      )}

      <div className="wtActions">
        <button onClick={() => setJoinOpen(true)}>Join Room</button>
        <button onClick={() => setSyncplayOpen(true)}>
          External Syncplay
        </button>
      </div>

      <JoinRoomModal visible={joinOpen} onClose={() => setJoinOpen(false)} />
      <ExternalSyncplayModal
        visible={syncplayOpen}
        onClose={() => setSyncplayOpen(false)}
      />
    </section>
  );
}

export default WatchTogether;
