import { useSelector } from "react-redux";

function ParticipantList() {
  const { participants } = useSelector((store) => store.watchTogether);

  return (
    <div className="wtParticipants">
      {participants.map((p) => (
        <div
          key={p.user_id}
          className={`wtParticipant ${p.is_buffering ? "buffering" : ""}`}
          title={`${p.username}${p.is_host ? " (host)" : ""}${p.is_buffering ? " (buffering)" : ""}${p.is_ready ? " (ready)" : ""}${p.client_type === "syncplay" ? " [Syncplay]" : ""}`}
        >
          <div className="wtAvatar">
            {p.picture ? (
              <img src={`/images/${p.picture}`} alt={p.username} />
            ) : (
              <span>{p.username.charAt(0).toUpperCase()}</span>
            )}
            {p.is_host && <span className="wtHostDot" />}
            {p.is_ready && <span className="wtReadyDot" />}
            {p.client_type === "syncplay" && (
              <span className="wtSyncplayBadge" title="Syncplay client" />
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

export default ParticipantList;
