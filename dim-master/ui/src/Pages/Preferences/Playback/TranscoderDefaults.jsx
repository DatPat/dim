import { useCallback, useState, useRef, useEffect } from "react";
import { useDispatch, useSelector } from "react-redux";
import { updateGlobalSettings } from "../../../actions/settings";

import "./TranscoderDefaults.scss";

function DetectedDevices() {
  const token = useSelector((store) => store.auth.token);
  const [devices, setDevices] = useState(null);

  useEffect(() => {
    let cancelled = false;

    fetch("/api/v1/host/devices", {
      headers: { Authorization: token },
    })
      .then((res) => res.ok ? res.json() : [])
      .then((data) => { if (!cancelled) setDevices(data); })
      .catch(() => { if (!cancelled) setDevices([]); });

    return () => { cancelled = true; };
  }, [token]);

  if (devices === null) return null;

  if (devices.length === 0) {
    return (
      <div className="deviceList">
        <p className="deviceListLabel">Detected GPU devices</p>
        <p className="noDevices">No GPU devices detected</p>
      </div>
    );
  }

  return (
    <div className="deviceList">
      <p className="deviceListLabel">Detected GPU devices</p>
      {devices.map((dev) => (
        <div className="deviceItem" key={dev.path}>
          <div className="deviceInfo">
            <span className="devicePath">{dev.path}</span>
            <span className="deviceVendor">{dev.vendor}</span>
          </div>
          <div className="deviceTags">
            <span className="tag kindTag">{dev.kind}</span>
            {dev.codecs.map((c) => (
              <span className="tag codecTag" key={c}>{c}</span>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

const hwaccelOptions = [
  { value: "off", label: "Off" },
  { value: "auto", label: "Auto-detect" },
  { value: "vaapi", label: "VAAPI (Intel/AMD)" },
  { value: "cuda", label: "CUDA (NVIDIA)" },
  { value: "qsv", label: "QSV (Intel Quick Sync)" },
  { value: "v4l2", label: "V4L2 (ARM)" },
  { value: "amf", label: "AMF (AMD, Windows)" },
];

const codecOptions = [
  { value: "h264", label: "H.264 (AVC)" },
  { value: "h265", label: "H.265 (HEVC)" },
  { value: "av1", label: "AV1" },
];

const decodeOptions = [
  { value: "auto", label: "Auto (hardware when available)" },
  { value: "software", label: "Software only" },
];

function SettingsDropdown({ label, options, value, onChange }) {
  const dropdownRef = useRef(null);
  const [visible, setVisible] = useState(false);

  const handleClick = useCallback((e) => {
    if (!dropdownRef.current) return;
    if (!dropdownRef.current.contains(e.target)) {
      setVisible(false);
    }
  }, []);

  useEffect(() => {
    window.addEventListener("click", handleClick);
    return () => window.removeEventListener("click", handleClick);
  }, [handleClick]);

  const selected = options.find((o) => o.value === value) || options[0];

  return (
    <div className="field">
      <p>{label}</p>
      <div className="dropdown" ref={dropdownRef}>
        <div
          className={`toggle visible-${visible}`}
          onClick={() => setVisible(!visible)}
        >
          <p>{selected.label}</p>
        </div>
        <div className={`dropDownContent visible-${visible}`}>
          {options
            .filter((o) => o.value !== value)
            .map((o) => (
              <button
                key={o.value}
                onClick={() => {
                  onChange(o.value);
                  setVisible(false);
                }}
              >
                {o.label}
              </button>
            ))}
        </div>
      </div>
    </div>
  );
}

function TranscoderDefaults() {
  const dispatch = useDispatch();

  const globalSettings = useSelector(
    (store) => store.settings.globalSettings.data
  );

  const hwaccel = globalSettings.enable_hwaccel || "off";
  const codec = globalSettings.default_video_codec || "h264";
  const decodeMethod = globalSettings.decode_method || "auto";

  const handleHwaccelChange = useCallback(
    (value) => {
      dispatch(updateGlobalSettings({ enable_hwaccel: value }));
    },
    [dispatch]
  );

  const handleCodecChange = useCallback(
    (value) => {
      dispatch(updateGlobalSettings({ default_video_codec: value }));
    },
    [dispatch]
  );

  const handleDecodeChange = useCallback(
    (value) => {
      dispatch(updateGlobalSettings({ decode_method: value }));
    },
    [dispatch]
  );

  return (
    <section className="transcoderDefaults">
      <h2>Transcoder Settings</h2>
      <DetectedDevices />
      <SettingsDropdown
        label="Hardware acceleration"
        options={hwaccelOptions}
        value={hwaccel}
        onChange={handleHwaccelChange}
      />
      <SettingsDropdown
        label="Default video codec"
        options={codecOptions}
        value={codec}
        onChange={handleCodecChange}
      />
      <SettingsDropdown
        label="Decode method"
        options={decodeOptions}
        value={decodeMethod}
        onChange={handleDecodeChange}
      />
    </section>
  );
}

export default TranscoderDefaults;
