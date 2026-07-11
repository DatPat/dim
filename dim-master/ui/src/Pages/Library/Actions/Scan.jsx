import { useCallback } from "react";
import { useDispatch, useSelector } from "react-redux";

import { scanLibrary } from "../../../actions/library";
import SearchIcon from "../../../assets/Icons/Search";

const Scan = (props) => {
  const dispatch = useDispatch();
  const scanning = useSelector((store) => store.library.scanning);
  const isScanning = scanning.includes(parseInt(props.id));

  const handleScan = useCallback(() => {
    if (isScanning) return;
    dispatch(scanLibrary(props.id));
  }, [isScanning, dispatch, props.id]);

  return (
    <button className="scan" onClick={handleScan} disabled={isScanning}>
      {isScanning ? "Scanning..." : "Scan library"}
      <SearchIcon />
    </button>
  );
};

export default Scan;
