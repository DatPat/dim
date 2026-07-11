import { useParams } from "react-router-dom";
import { useCallback, useState } from "react";

import { useGetMediaQuery, useGetMediaFilesQuery } from "../../api/v1/media";

import Banner from "./Banner";
import MetaContent from "./MetaContent";
import Seasons from "./Seasons";
import Button from "../../Components/Misc/Button";
import CreateRoomModal from "../../Modals/WatchTogether/CreateRoomModal";
import JoinRoomModal from "../../Modals/WatchTogether/JoinRoomModal";
import ExternalSyncplayModal from "../../Modals/WatchTogether/ExternalSyncplayModal";

import "./Index.scss";

function Media() {
  const { id } = useParams();
  const [activeId, setActiveId] = useState(id);
  const { data: media } = useGetMediaQuery(id);
  const { data: mediaFiles } = useGetMediaFilesQuery(id);

  const [showCreateRoom, setShowCreateRoom] = useState(false);
  const [showJoinRoom, setShowJoinRoom] = useState(false);
  const [showExternalSync, setShowExternalSync] = useState(false);

  const handleOpenCreate = useCallback(() => setShowCreateRoom(true), []);
  const handleCloseCreate = useCallback(() => setShowCreateRoom(false), []);
  const handleOpenJoin = useCallback(() => setShowJoinRoom(true), []);
  const handleCloseJoin = useCallback(() => setShowJoinRoom(false), []);
  const handleOpenExternal = useCallback(() => setShowExternalSync(true), []);
  const handleCloseExternal = useCallback(() => setShowExternalSync(false), []);

  const firstFileId = mediaFiles && mediaFiles.length > 0 ? mediaFiles[0].id : null;

  return (
    <div className="mediaPage">
      <Banner />
      <div className="mediaContent">
        <div className="meta-content">
          <MetaContent activeId={activeId} />
          <div style={{ display: "flex", gap: 8, marginTop: 10, flexWrap: "wrap" }}>
            {media && media.media_type !== "tv" && firstFileId && (
              <Button onClick={handleOpenCreate}>
                <p>Watch Together</p>
              </Button>
            )}
            <Button onClick={handleOpenJoin}>
              <p>Join Room</p>
            </Button>
            <Button onClick={handleOpenExternal}>
              <p>External Syncplay</p>
            </Button>
          </div>
        </div>
        {media && media.media_type === "tv" && (
          <Seasons setActiveId={setActiveId} />
        )}
      </div>
      {firstFileId && (
        <CreateRoomModal
          visible={showCreateRoom}
          onClose={handleCloseCreate}
          mediaFileId={firstFileId}
          mediaId={parseInt(id)}
          mediaName={media?.name || ""}
        />
      )}
      <JoinRoomModal visible={showJoinRoom} onClose={handleCloseJoin} />
      <ExternalSyncplayModal visible={showExternalSync} onClose={handleCloseExternal} />
    </div>
  );
}

export default Media;
