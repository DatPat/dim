import { useCallback, useEffect, useRef, useState, useContext } from "react";
import { useDispatch, useSelector } from "react-redux";

import { updateVideo, updateTrack } from "../../../actions/video";

import { VideoPlayerContext } from "../Context";

import ArrowLeftIcon from "../../../assets/Icons/ArrowLeft";
import ChevronRightIcon from "../../../assets/Icons/ChevronRight";

function classifyProfile(session) {
  if (!session) return null;

  var is_direct = session.is_direct;
  var profile_tag = session.profile_tag;
  var profile_type = session.profile_type;

  if (is_direct || profile_type === "Transmux") {
    return { method: "Direct Play", color: "#4caf50" };
  }

  if (profile_type === "HardwareTranscode") {
    var tag = (profile_tag || "").toLowerCase();
    if (tag.includes("nvenc") || tag.includes("cuda")) {
      return { method: "NVIDIA NVENC", color: "#76b900" };
    }
    if (tag.includes("vaapi")) {
      return { method: "VAAPI", color: "#0071c5" };
    }
    if (tag.includes("qsv")) {
      return { method: "Intel QSV", color: "#0071c5" };
    }
    return { method: "Hardware", color: "#ff9800" };
  }

  if (profile_type === "Transcode") {
    return { method: "Software", color: "#ff5722" };
  }

  return { method: "Transcoding", color: "#ff9800" };
}

function StreamInfoSection() {
  var gid = useSelector(function (store) { return store.video.gid; });
  var auth = useSelector(function (store) { return store.auth; });
  var showSettings = useSelector(function (store) { return store.video.showSettings; });
  var videoTracks = useSelector(function (store) { return store.video.tracks.video; });
  var [info, setInfo] = useState(null);

  // The currently selected video track's session id. `current` can be -1 when
  // the active track hasn't been resolved yet.
  var activeTrack = videoTracks && videoTracks.list && videoTracks.current >= 0
    ? videoTracks.list[videoTracks.current]
    : null;
  var activeSessionId = activeTrack ? activeTrack.id : null;

  useEffect(function () {
    if (!gid || !showSettings) return;

    var cancelled = false;

    function fetchInfo() {
      fetch("/api/v1/stream/" + gid + "/state/stream_info", {
        headers: { authorization: auth.token },
      })
        .then(function (res) {
          if (!res.ok || cancelled) return null;
          return res.json();
        })
        .then(function (data) {
          if (cancelled || !data) return;

          var videoSession = null;
          if (data.sessions) {
            // Match the currently active video track's session
            if (activeSessionId) {
              for (var i = 0; i < data.sessions.length; i++) {
                if (data.sessions[i].session_id === activeSessionId) {
                  videoSession = data.sessions[i];
                  break;
                }
              }
            }
            // Fallback: first video session
            if (!videoSession) {
              for (var j = 0; j < data.sessions.length; j++) {
                if (data.sessions[j].content_type === "video") {
                  videoSession = data.sessions[j];
                  break;
                }
              }
            }
          }

          if (videoSession) {
            setInfo(videoSession);
          }
        })
        .catch(function () {});
    }

    fetchInfo();
    var interval = setInterval(fetchInfo, 3000);

    return function () {
      cancelled = true;
      clearInterval(interval);
    };
  }, [gid, showSettings, auth.token, activeSessionId]);

  if (!info) return null;

  var classification = classifyProfile(info);
  if (!classification) return null;

  var speed = info.speed ? info.speed.replace(/\s/g, "") : null;
  var fps = info.fps ? info.fps.replace(/\s/g, "") : null;

  var decoder = info.decoder || null;
  var inputCodec = info.input_codec ? info.input_codec.toUpperCase() : null;

  return (
    <>
      <div className="streamInfoRow">
        <span className="streamInfoLabel">Encoder</span>
        <span className="streamInfoValue">
          <span style={{ color: classification.color }}>
            {classification.method}
          </span>
          {fps && (
            <span className="streamInfoStat">{fps} fps</span>
          )}
          {speed && !info.is_direct && (
            <span className="streamInfoStat">{speed}</span>
          )}
        </span>
      </div>
      {decoder && (
        <div className="streamInfoRow">
          <span className="streamInfoLabel">Decoder</span>
          <span className="streamInfoValue">
            <span>{decoder}</span>
            {inputCodec && (
              <span className="streamInfoStat">{inputCodec}</span>
            )}
          </span>
        </div>
      )}
    </>
  );
}

function VideoMenuSettings() {
  const dispatch = useDispatch();

  const { player } = useContext(VideoPlayerContext);

  const { video } = useSelector((store) => ({
    video: store.video,
  }));

  const [activeInnerMenu, setActiveInnerMenu] = useState();

  const menuRef = useRef(null);

  const handleClick = useCallback(
    (e) => {
      if (!menuRef.current || e.target.nodeName !== "DIV") return;

      if (!menuRef.current.contains(e.target)) {
        dispatch(
          updateVideo({
            showSettings: false,
          })
        );
      }
    },
    [dispatch]
  );

  const goBack = useCallback(() => {
    if (!activeInnerMenu) return;
    setActiveInnerMenu();
  }, [activeInnerMenu]);

  const changeTrack = useCallback(
    (trackType, i) => {
      const tracks =
        trackType === "video"
          ? video.tracks.video.list
          : video.tracks.audio.list;

      const playerTracks = player.getTracksFor(trackType);
      const selectedTrack = playerTracks.filter(
        (track) => track.id === tracks[i].set_id
      );

      console.log("[video] changed track to", selectedTrack[0]);

      player.setCurrentTrack(selectedTrack[0]);

      dispatch(
        updateTrack(trackType, {
          current: parseInt(i),
        })
      );
    },
    [dispatch, player, video]
  );

  useEffect(() => {
    window.addEventListener("click", handleClick);

    return () => {
      window.removeEventListener("click", handleClick);
    };
  }, [handleClick]);

  window.video = video;
  return (
    <div className="menu" ref={menuRef}>
      <div className="heading">
        <h3>{activeInnerMenu ? activeInnerMenu : "Settings"}</h3>
        {activeInnerMenu && (
          <button onClick={goBack}>
            <ArrowLeftIcon />
          </button>
        )}
      </div>
      <div className="separatorContainer">
        <div className="separator" />
      </div>
      {activeInnerMenu === undefined && (
        <div className="innerMenus">
          <StreamInfoSection />
          <p onClick={() => setActiveInnerMenu("Video Quality")}>
            Video tracks
            <ChevronRightIcon />
          </p>
          <p onClick={() => setActiveInnerMenu("Audio tracks")}>
            Audio tracks
            <ChevronRightIcon />
          </p>
        </div>
      )}
      {activeInnerMenu === "Video Quality" && (
        <div className="innerMenu">
          <div className="tracks">
            {video.tracks.video.list.map((track, i) => (
              <div
                key={i}
                className={`track ${
                  video.tracks.video.current === i ? "active" : ""
                }`}
                onClick={() => changeTrack("video", `${i}`)}
              >
                <p>{track.label}</p>
              </div>
            ))}
          </div>
        </div>
      )}
      {activeInnerMenu === "Audio tracks" && (
        <div className="innerMenu">
          <div className="tracks">
            {video.tracks.audio.list.map((track, i) => (
              <div
                key={i}
                className={`track ${
                  video.tracks.audio.current === i ? "active" : ""
                }`}
                onClick={() => changeTrack("audio", `${i}`)}
              >
                <p>{track.label}</p>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export default VideoMenuSettings;
