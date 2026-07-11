import { useCallback } from "react";
import { NavLink } from "react-router-dom";
import { useDispatch } from "react-redux";

import { useAppSelector } from "hooks/store";
import { scanLibrary } from "actions/library.js";
import FilmIcon from "assets/Icons/Film";
import TvIcon from "assets/Icons/TvIcon";
import RefreshIcon from "assets/Icons/Refresh";
import BarLoad from "Components/Load/Bar";

interface Props {
  id: string;
  media_type: string;
  name: string;
}

function Library(props: Props) {
  const dispatch = useDispatch();
  const scanning = useAppSelector((store) => store.library.scanning);
  const user = useAppSelector((store: any) => store.user);
  const { id, media_type, name } = props;
  const isScanning = scanning.includes(id);
  const isOwner = user.info.roles?.includes("owner");

  const handleScan = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (isScanning) return;
      dispatch(scanLibrary(id) as any);
    },
    [dispatch, id, isScanning]
  );

  return (
    <NavLink
      to={"/library/" + id}
      className={`item showLoad-${isScanning}`}
    >
      {media_type === "movie" && <FilmIcon />}
      {media_type === "tv" && <TvIcon />}
      <p>{name}</p>
      {isOwner && (
        <button
          className={`scanBtn${isScanning ? " scanning" : ""}`}
          onClick={handleScan}
          disabled={isScanning}
          title={isScanning ? "Scanning..." : "Scan library"}
        >
          <RefreshIcon />
        </button>
      )}
      {isScanning && <BarLoad />}
    </NavLink>
  );
}

export default Library;
