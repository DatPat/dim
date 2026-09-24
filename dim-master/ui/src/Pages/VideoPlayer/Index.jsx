import { useCallback, useContext, useEffect, useRef, useState } from "react";
import { useParams } from "react-router";
import { useHistory } from "react-router-dom";
import { useDispatch, useSelector, useStore } from "react-redux";
import { skipToken } from "@reduxjs/toolkit/query/react";
import { MediaPlayer, Debug } from "dashjs";
import { WebSocketContext } from "../../Components/WS";
import {
  setTracks,
  setGID,
  setManifestState,
  updateVideo,
  incIdleCount,
  clearVideoData,
} from "../../actions/video";
import { fetchUserSettings } from "../../actions/settings.js";
import useRoomLifecycle from "./WatchTogether/useRoomLifecycle";
import useEpisodeNavigation from "./WatchTogether/useEpisodeNavigation";
import { useGetMediaFilesQuery, useGetMediaQuery } from "../../api/v1/media";
import { VideoPlayerContext } from "./Context";
import VideoEvents from "./Events";
import StallGuard from "./StallGuard";
import ErrorBoundary from "../../Components/ErrorBoundary";
import VideoMediaData from "./MediaData";

import RingLoad from "../../Components/Load/Ring";
import Menus from "./Menus/Index";
import VideoControls from "./Controls/Index";
import ArrowLeftIcon from "../../assets/Icons/ArrowLeft";
import ErrorBox from "./ErrorBox";
import ContinueProgress from "./ContinueProgress";
import VttSubtitles from "./VttSubtitles";
import SsaSubtitles from "./SsaSubtitles";
import NextVideo from "./NextVideo/Index";
import ReturnToShow from "./NextVideo/ReturnToShow";
import SyncController from "./WatchTogether/SyncController";
import ExternalSyncController from "./WatchTogether/ExternalSyncController";
import RoomOverlay from "./WatchTogether/RoomOverlay";
import ExternalSyncOverlay from "./WatchTogether/ExternalSyncOverlay";
import ChatBubbles from "./WatchTogether/ChatBubbles";

import "./Index.scss";

function VideoPlayer() {
  const params = useParams();
  const dispatch = useDispatch();
  const history = useHistory();
  const [player, setPlayer] = useState();
  const manifestFileID = useRef(null);
  const playerFileID = useRef(null);
  const playNextFile = useEpisodeNavigation(params.fileID);

  const wtState = useSelector((store) => store.watchTogether);
  useRoomLifecycle(params.fileID);
  const activeBuiltinRoom = wtState.isInRoom && !wtState.externalSync &&
    String(wtState.mediaFileId) === String(params.fileID);

  const { error, manifest, audioTracks, videoTracks, video, auth, settings } =
    useSelector((store) => ({
      auth: store.auth,
      video: store.video,
      manifest: store.video.manifest,
      videoTracks: store.video.tracks.video,
      audioTracks: store.video.tracks.audio,
      error: store.video.error,
      settings: store.settings,
    }));

  const ws = useContext(WebSocketContext);

  const videoPlayer = useRef(null);
  const overlay = useRef(null);
  const videoRef = useRef(null);

  const { token } = auth;
  const canBroadcast = activeBuiltinRoom && wtState.roomCode &&
    (wtState.isHost || wtState.controlMode === "egalitarian");

  const { data: media } = useGetMediaQuery(
    video.mediaID ? video.mediaID : skipToken
  );
  const nextEpisodeId = media && media.next_episode_id;
  const { data: nextMediaFiles } = useGetMediaFilesQuery(
    nextEpisodeId ? nextEpisodeId : skipToken
  );

  useEffect(() => {
    if (media) {
      document.title = `Dim - Playing '${media.name}'`;
    } else {
      document.title = "Dim - Video Player";
    }
  }, [media]);

  // FIXME: Not sure where the best place to do this is, but we need userSettings, but sometimes the user navigates to /play directly so we never fetch userSettings
  useEffect(() => {
    if (settings.userSettings.fetching || settings.userSettings.fetched) return;

    dispatch(fetchUserSettings());
  }, [dispatch, settings.userSettings]);

  // Flush the final playback position when the player closes. SeekBar only
  // saves every 15s, so without this, quitting during an outro can strand
  // the item just short of the server's watched threshold. Declared before
  // the dash.js effect so this cleanup reads redux before clearVideoData
  // wipes it. keepalive lets the request survive tab close / navigation.
  const reduxStore = useStore();
  useEffect(() => {
    const flushProgress = () => {
      const { video: v, auth: a } = reduxStore.getState();
      const id = v.episode?.id || v.mediaID;
      if (!id || !v.currentTime) return;

      fetch(`/api/v1/media/${id}/progress?offset=${v.currentTime}`, {
        method: "POST",
        headers: { authorization: a.token },
        keepalive: true,
      });
    };

    window.addEventListener("beforeunload", flushProgress);
    return () => {
      window.removeEventListener("beforeunload", flushProgress);
      flushProgress();
    };
  }, [reduxStore]);

  // If playback finished, redirect to the next video
  useEffect(() => {
    if (!settings?.userSettings?.data?.enable_autoplay) return;

    const item = nextMediaFiles && nextMediaFiles[0];

    if (!item) return;

    if (!media) return;

    const ts_diff = video.currentTime - media.duration;
    if (video.playback_ended && ts_diff < 10) {
      playNextFile(item.id);
    }
  }, [
    media,
    nextMediaFiles,
    video.mediaID,
    video.currentTime,
    video.playback_ended,
    history,
    settings,
    settings.userSettings,
    playNextFile,
  ]);

  // Reset GID if play id changes so that this component loads a new video.
  useEffect(() => {
    dispatch(setGID(null));
  }, [params.fileID, dispatch]);

  useEffect(() => {
    if (video.gid) return;

    const force_ass = localStorage.getItem("enable_ssa") === "true";

    const supportedCodecs = [];
    if (typeof MediaSource !== "undefined") {
      if (MediaSource.isTypeSupported('video/mp4; codecs="avc1.640028"')) supportedCodecs.push("h264");
      // Browsers disagree on which HEVC fourcc they report support under
      // (Safari: hvc1, Chromium w/ platform decode: either) — probe both.
      if (
        MediaSource.isTypeSupported('video/mp4; codecs="hev1.1.6.L120.90"') ||
        MediaSource.isTypeSupported('video/mp4; codecs="hvc1.1.6.L120.90"')
      ) supportedCodecs.push("h265");
      if (MediaSource.isTypeSupported('video/mp4; codecs="av01.0.08M.08"')) supportedCodecs.push("av1");
      // VP9-in-fMP4 (vp09 sample entries) — Chromium/Firefox support this via
      // MSE; enables direct play of VP9 sources instead of forced transcode.
      if (MediaSource.isTypeSupported('video/mp4; codecs="vp09.00.40.08"')) supportedCodecs.push("vp9");
    }

    const host = `/api/v1/stream/${params.fileID}/manifest?force_ass=${force_ass}&supported_codecs=${supportedCodecs.join(",")}`;

    (async () => {
      const config = {
        headers: {
          authorization: token,
        },
      };

      // A failed manifest request (corrupt file, server error) used to throw
      // out of this async block unhandled — the player hung on the loading
      // spinner forever with no message.
      let res;
      try {
        res = await fetch(host, config);
      } catch (err) {
        dispatch(updateVideo({ error: { msg: `Failed to load stream: ${err}`, errors: [] } }));
        return;
      }

      if (!res.ok) {
        let detail = "";
        try {
          const body = await res.json();
          detail = body.error || body.messsage || body.message || JSON.stringify(body);
        } catch {}
        dispatch(
          updateVideo({
            error: {
              msg: `The server could not prepare this file for playback (HTTP ${res.status}). ${detail}`,
              errors: [],
            },
          })
        );
        return;
      }

      const payload = await res.json();

      manifestFileID.current = params.fileID;
      dispatch(setGID(payload.gid));

      // Log transcode reasons for each track
      console.group("[dim] Transcode decisions");
      for (const track of payload.tracks) {
        if (track.is_direct) {
          console.log(`%c${track.content_type} [${track.label}]: Direct Play`, "color: #4caf50");
        } else if (track.transcode_reason) {
          console.log(`%c${track.content_type} [${track.label}]: ${track.transcode_reason}`, "color: #ff9800");
        }
      }
      console.groupEnd();

      const tVideos = payload.tracks.filter(
        (track) => track.content_type === "video"
      );
      const tAudios = payload.tracks.filter(
        (track) => track.content_type === "audio"
      );
      const tSubtitles = payload.tracks.filter(
        (track) => track.content_type === "subtitle"
      );

      dispatch(
        setTracks({
          video: tVideos,
          audio: tAudios,
          subtitle: tSubtitles,
        })
      );

      dispatch(
        setManifestState({
          virtual: { loaded: true },
        })
      );
    })();
  }, [dispatch, params.fileID, token, video.gid]);

  useEffect(() => {
    if (!video.gid || !manifest.virtual.loaded) return;

    console.log("[video] loading manifest");

    dispatch(
      setManifestState({
        loading: true,
        loaded: false,
      })
    );

    const includes = `${videoTracks.list
      .map((track) => track.id)
      .join(",")},${audioTracks.list.map((track) => track.id).join(",")}`;
    const url = `/api/v1/stream/${video.gid}/manifest.mpd?start_num=0&should_kill=false&includes=${includes}`;
    const mediaPlayer = MediaPlayer().create();

    let settings = {
      debug: {
        logLevel: Debug.LOG_LEVEL_WARNING,
      },
      streaming: {
        /* FIXME: Disabling temporarily because the code for this function is unsound
        gaps: {
          enableSeekFix: true
        },
        */
        // Jump over timeline gaps instead of stalling on them. dash.js 4.1's
        // default leaves jumpLargestGap OFF, so any gap wider than the small
        // gap limit (short transmux segment, patch boundary) froze playback
        // permanently.
        gaps: {
          jumpGaps: true,
          jumpLargestGap: true,
          smallGapLimit: 1.5,
          threshold: 0.3,
        },
        abr: {
          autoSwitchBitrate: {
            video: false,
          },
        },
      },
    };

    mediaPlayer.updateSettings(settings);
    mediaPlayer.extend("RequestModifier", function () {
      return {
        modifyRequestHeader: function (xhr) {
          xhr.setRequestHeader("Authorization", auth.token);
          return xhr;
        },
        modifyRequestURL: function (url) {
          return url;
        },
      };
    });

    const getInitialTrack = (trackArr) => {
      const trackList =
        trackArr[0].type === "video" ? videoTracks.list : audioTracks.list;
      const defaultTracks = trackList.filter((track) => track.is_default);
      const defaultTrack =
        defaultTracks && defaultTracks.length > 0
          ? defaultTracks[0]
          : trackList[0];
      const initialTracks = trackArr.filter(
        (x) => x.id === defaultTrack.set_id
      );
      console.log(
        `[${trackArr[0].type}] setting initial track to`,
        initialTracks
      );
      return initialTracks;
    };

    // For external sync guests, mute before initialization so dash.js
    // autoplay isn't blocked by the browser's autoplay policy.
    if (wtState.externalSync && wtState.externalIsGuest && videoRef.current) {
      videoRef.current.muted = true;
    }

    // Don't autoplay if we're in a Watch Together room where the host is paused
    // — unless we are the host that just advanced the room to this episode
    // (the room restarts paused until our player reports it playing).
    const hostAdvancedHere = wtState.isHost &&
      String(wtState.autoplayFileId) === String(params.fileID);
    const shouldAutoplay = hostAdvancedHere ||
      !(wtState.isInRoom && wtState.playbackState === "paused");
    mediaPlayer.initialize(videoRef.current, url, shouldAutoplay);
    // Keep redux in sync with the autoplay decision. The reducer's initial
    // state is `paused: false`, which is only ever corrected by dash.js
    // playback events — and a player that initializes PAUSED never fires
    // one. Without this, a paused WT room shows the pause icon on a paused
    // video and the play button is dead (pause() on a paused element fires
    // no event to self-correct through).
    dispatch(updateVideo({ paused: !shouldAutoplay }));
    mediaPlayer.setCustomInitialTrackSelectionFunction(getInitialTrack);

    playerFileID.current = manifestFileID.current;
    setPlayer(mediaPlayer);

    return () => {
      dispatch(clearVideoData());
      // dash.js 4.1.0's destroy() synchronously flushes buffer-state events
      // into controllers whose streamInfo is already nulled and THROWS
      // (PlaybackController._onBufferLevelStateChanged). Uncaught, that
      // exception propagates through this React cleanup and unmounts the
      // entire app — the "player froze / went black" failure.
      try {
        mediaPlayer.destroy();
      } catch (e) {
        console.warn("[video] dash.js destroy threw (ignored):", e);
      }

      if (!video.gid) return;

      (async () => {
        await fetch(`/api/v1/stream/${video.gid}/state/kill`);
        sessionStorage.clear();
      })();
    };
  }, [
    audioTracks.list,
    auth.token,
    dispatch,
    manifest.virtual.loaded,
    video.gid,
    videoTracks.list,
    setPlayer,
  ]);

  const isWtNonHost = wtState.externalSync ? wtState.externalIsGuest : (wtState.isInRoom && wtState.controlMode === "host_only" && !wtState.isHost);

  const play = useCallback(() => {
    if (isWtNonHost) return;
    dispatch(
      updateVideo({
        idleCount: 0,
      })
    );

    videoRef.current.play();

    if (ws && canBroadcast) {
      ws.send(JSON.stringify({
        type: "watch_together_play",
        room_code: wtState.roomCode,
        position: videoRef.current.currentTime,
      }));
    }
  }, [dispatch, videoRef, isWtNonHost, ws, canBroadcast, wtState.roomCode]);

  const pause = useCallback(() => {
    if (isWtNonHost) return;
    dispatch(
      updateVideo({
        idleCount: 0,
      })
    );
    videoRef.current.pause();

    if (ws && canBroadcast) {
      ws.send(JSON.stringify({
        type: "watch_together_pause",
        room_code: wtState.roomCode,
        position: videoRef.current.currentTime,
      }));
    }
  }, [dispatch, videoRef, isWtNonHost, ws, canBroadcast, wtState.roomCode]);

  const togglePlayer = useCallback(
    (e) => {
      if (isWtNonHost) return;
      if (!videoRef.current) return;
      if (
        e.target.closest(
          ".videoMenus, .videoControls, .videoCloseButton, .modalBoxContainer, .ReactModalPortal, .wtOverlay"
        )
      )
        return;

      videoRef.current.paused ? play() : pause();
    },
    [play, pause, videoRef, isWtNonHost]
  );

  const closePlayer = useCallback(() => {
    // Land on the page of what was just watched (the show for episodes,
    // the movie itself otherwise) — not wherever browser history happens
    // to point, which after autoplay is just the previous episode's URL.
    const mediaPageId = media && (media.tv_show_id || media.id);
    if (mediaPageId) {
      history.push(`/media/${mediaPageId}`);
    } else if (history.length > 1) {
      history.goBack();
    } else {
      history.push("/");
    }
  }, [history, media]);

  useEffect(() => {
    const handleKeyDown = (e) => {
      if (e.key !== "Escape") return;
      // Don't intercept Escape if a modal is open or a menu is showing.
      if (document.querySelector(".ReactModalPortal .ReactModal__Content")) return;
      if (document.fullscreenElement) return;
      e.preventDefault();
      closePlayer();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [closePlayer]);

  const seekTo = useCallback(
    (newTime) => {
      if (isWtNonHost) return;
      player.seek(newTime);

      dispatch(
        updateVideo({
          seeking: false,
          currentTime: newTime,
        })
      );

      if (ws && canBroadcast) {
        ws.send(JSON.stringify({
          type: "watch_together_seek",
          room_code: wtState.roomCode,
          position: newTime,
        }));
      }
    },
    [dispatch, player, isWtNonHost, ws, canBroadcast, wtState.roomCode]
  );

  useEffect(() => {
    if (video.showSubSwitcher) return;
    dispatch(incIdleCount());
  }, [video.currentTime, dispatch, video.showSubSwitcher]);

  const initialValue = {
    videoRef,
    videoPlayer,
    overlay: overlay.current,
    seekTo,
    player: playerFileID.current === params.fileID &&
      (!wtState.isInRoom || wtState.externalSync || activeBuiltinRoom) ? player : null,
  };

  const showNextVideoAfter = (media && media.chapters?.credits) || 0;

  return (
    <VideoPlayerContext.Provider value={initialValue}>
      <div className="videoPlayer" ref={videoPlayer} onClick={togglePlayer}>
        <VideoEvents />
        <StallGuard />
        <VideoMediaData />
        {wtState.isInRoom && !wtState.externalSync && <SyncController />}
        {wtState.externalSync && <ExternalSyncController />}
        <video ref={videoRef} />
        <VttSubtitles />
        <SsaSubtitles />
        <div className="overlay" ref={overlay}>
          <button
            type="button"
            className={`videoCloseButton ${video.idleCount <= 2 ? "true" : "false"}`}
            onClick={closePlayer}
            aria-label="Close player"
            title="Close player (Esc)"
          >
            <ArrowLeftIcon />
          </button>
          {wtState.isInRoom && !wtState.externalSync && !error && manifest.loaded && video.canPlay && (
            <RoomOverlay />
          )}
          {wtState.externalSync && (
            <ExternalSyncOverlay />
          )}
          <ChatBubbles />
          {!error && manifest.loaded && video.canPlay && <Menus />}
          {!error && manifest.loaded && video.canPlay && nextEpisodeId &&
            (!wtState.isInRoom || wtState.externalSync || wtState.isHost) && (
            <NextVideo id={nextEpisodeId} showAfter={showNextVideoAfter} onSelectFile={playNextFile} />
          )}
          {!error &&
            manifest.loaded &&
            video.canPlay &&
            !nextEpisodeId &&
            media?.tv_show_id && (
              <ReturnToShow
                showId={media.tv_show_id}
                showAfter={showNextVideoAfter}
              />
            )}
          {!error && manifest.loaded && video.canPlay && <VideoControls />}
          {(!error & (manifest.loading || !video.canPlay) || video.waiting) && (
            <RingLoad />
          )}
          {!error &&
            manifest.loaded &&
            video.canPlay &&
            media &&
            media.progress > 0 &&
            !wtState.isInRoom &&
            !wtState.externalSync && <ContinueProgress />}
          {error && <ErrorBox />}
        </div>
      </div>
    </VideoPlayerContext.Provider>
  );
}

// A render crash anywhere in the player must not blank the whole app —
// without a boundary React unmounts the entire tree, which users experience
// as playback silently freezing.
function VideoPlayerPage() {
  return (
    <ErrorBoundary>
      <VideoPlayer />
    </ErrorBoundary>
  );
}

export default VideoPlayerPage;
