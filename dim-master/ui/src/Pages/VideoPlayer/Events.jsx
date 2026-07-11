import { useCallback, useEffect, useContext } from "react";
import { useDispatch, useSelector } from "react-redux";
import { MediaPlayer } from "dashjs";

import { VideoPlayerContext } from "./Context";

import {
  setManifestState,
  updateTrack,
  updateVideo,
} from "../../actions/video";

function VideoEvents() {
  const dispatch = useDispatch();
  const { player } = useContext(VideoPlayerContext);

  const { video } = useSelector((store) => ({
    video: store.video,
  }));
  const authToken = useSelector((store) => store.auth.token);

  const eManifestLoad = useCallback(() => {
    console.log("[VIDEO] manifest loaded");

    dispatch(
      setManifestState({
        loading: false,
        loaded: true,
      })
    );
  }, [dispatch]);

  const videoTrackList = video.tracks.video.list;
  const audioTrackList = video.tracks.audio.list;

  const eCanPlay = useCallback(() => {
    console.log("[VIDEO] can play");

    // Each quality tier is its own DASH AdaptationSet, so the active track is
    // identified by set_id. Fall back to bitrate/height matching just in case.
    const videoTrack = player.getCurrentTrackFor("video");
    let videoTrackIdx = videoTrack
      ? videoTrackList.findIndex((track) => track.set_id === videoTrack.id)
      : -1;

    if (videoTrackIdx === -1) {
      const videoQualityIndex = player.getQualityFor("video");
      const videoQuality =
        player.getBitrateInfoListFor("video")[videoQualityIndex];

      if (videoQuality) {
        videoTrackIdx = videoTrackList.findIndex(
          (track) =>
            track.bandwidth === videoQuality.bitrate &&
            parseInt(track.height) === videoQuality.height
        );
      }
    }

    const audioTrack = player.getCurrentTrackFor("audio");
    const audioTrackIdx = audioTrack
      ? audioTrackList.findIndex((track) => track.set_id === audioTrack.id)
      : -1;

    if (videoTrackIdx !== -1) {
      dispatch(
        updateTrack("video", {
          current: videoTrackIdx,
        })
      );
    }

    if (audioTrackIdx !== -1) {
      dispatch(
        updateTrack("audio", {
          current: audioTrackIdx,
        })
      );
    }

    dispatch(
      updateVideo({
        canPlay: true,
        waiting: false,
        duration: Math.round(player.duration()) | 0,
      })
    );
  }, [dispatch, player, videoTrackList, audioTrackList]);

  const ePlayBackPaused = useCallback(() => {
    console.log("[VIDEO] paused");

    dispatch(
      updateVideo({
        paused: true,
      })
    );
  }, [dispatch]);

  const ePlayBackPlaying = useCallback(() => {
    dispatch(
      updateVideo({
        paused: false,
      })
    );
  }, [dispatch]);

  const ePlayBackWaiting = useCallback(() => {
    console.log("[VIDEO] playback waiting");

    dispatch(
      updateVideo({
        waiting: true,
      })
    );
  }, [dispatch]);

  const ePlayBackEnded = useCallback(() => {
    console.log("[VIDEO] playback ended");

    dispatch(
      updateVideo({
        playback_ended: true,
      })
    );
  }, [dispatch]);

  const eError = useCallback(
    (e) => {
      // segment not available — transient; the encoder is usually just
      // behind. The stall guard handles the case where it never recovers.
      if (e.error.code === 27) {
        console.log("[VIDEO] segment not available", e.error.message);
        return;
      }

      (async () => {
        console.log("[VIDEO] fetching stderr");
        // NOTE: must always end in an error dispatch — this fetch used to
        // run without an Authorization header, 401, throw on .json(), and
        // leave the player frozen with no error UI at all.
        let errors = [];
        try {
          const res = await fetch(
            `/api/v1/stream/${video.gid}/state/get_stderr`,
            { headers: { authorization: authToken } }
          );
          if (res.ok) {
            errors = (await res.json()).errors;
          }
        } catch {
          // stderr is best-effort diagnostics only
        }

        dispatch(
          updateVideo({
            error: {
              msg: e.error.message,
              errors,
            },
          })
        );
      })();
    },
    [dispatch, video.gid, authToken]
  );

  const ePlayBackNotAllowed = useCallback(
    (e) => {
      console.log("[VIDEO] playback not allowed");

      if (e.type === "playbackNotAllowed") {
        dispatch(
          updateVideo({
            paused: true,
          })
        );
      }
    },
    [dispatch]
  );

  /*
    PLAYBACK_PROGRESS event stops after error occurs
    so using this event from now on to get buffer length
  */
  const ePlayBackTimeUpdated = useCallback(
    (e) => {
      /*
      on some browsers (*cough*, chrome) current
      time gets reset back to 0 on seek
    */
      let newTime = Math.floor(e.time);

      if (newTime < video.prevSeekTo) {
        newTime += video.prevSeekTo - newTime;
      }

      dispatch(
        updateVideo({
          currentTime: newTime,
          buffer: Math.round(player.getBufferLength()),
          waiting: false,
        })
      );
    },
    [dispatch, player, video.prevSeekTo]
  );

  const eQualityChange = useCallback(
    (e) => {
      console.log("[video] quality changing ", e);

      if (e.mediaType !== "video") return;

      // The active AdaptationSet identifies the track; bitrate/height matching
      // is only a fallback since tiers can collide on those values.
      const current = player.getCurrentTrackFor("video");
      let idx = current
        ? videoTrackList.findIndex((track) => track.set_id === current.id)
        : -1;

      if (idx === -1) {
        const newTrack = player.getBitrateInfoListFor("video")[e.newQuality];
        if (newTrack) {
          idx = videoTrackList.findIndex(
            (track) =>
              track.bandwidth === newTrack.bitrate &&
              parseInt(track.height) === newTrack.height
          );
        }
      }

      if (idx !== -1) {
        dispatch(
          updateTrack("video", {
            current: idx,
          })
        );
      }
    },
    [dispatch, player, videoTrackList]
  );

  const eTrackChange = useCallback(
    (e) => {
      console.log("[video] track changing ", e);

      if (e.mediaType !== "audio" && e.mediaType !== "video") return;

      const tracks = e.mediaType === "video" ? videoTrackList : audioTrackList;
      const idx = tracks.findIndex(
        (track) => track.set_id === e.newMediaInfo.id
      );

      if (idx !== -1) {
        dispatch(
          updateTrack(e.mediaType, {
            current: idx,
          })
        );
      }
    },
    [dispatch, videoTrackList, audioTrackList]
  );

  // other events
  useEffect(() => {
    if (!player) return;

    player.on(MediaPlayer.events.MANIFEST_LOADED, eManifestLoad);
    player.on(MediaPlayer.events.CAN_PLAY, eCanPlay);
    player.on(MediaPlayer.events.ERROR, eError);

    return () => {
      player.off(MediaPlayer.events.MANIFEST_LOADED, eManifestLoad);
      player.off(MediaPlayer.events.CAN_PLAY, eCanPlay);
      player.off(MediaPlayer.events.ERROR, eError);
    };
  }, [eCanPlay, eError, eManifestLoad, player]);

  // video playback
  useEffect(() => {
    if (!player) return;

    player.on(MediaPlayer.events.PLAYBACK_PAUSED, ePlayBackPaused);
    player.on(MediaPlayer.events.PLAYBACK_PLAYING, ePlayBackPlaying);
    player.on(MediaPlayer.events.PLAYBACK_WAITING, ePlayBackWaiting);
    player.on(MediaPlayer.events.PLAYBACK_TIME_UPDATED, ePlayBackTimeUpdated);
    player.on(MediaPlayer.events.PLAYBACK_NOT_ALLOWED, ePlayBackNotAllowed);
    player.on(MediaPlayer.events.PLAYBACK_ENDED, ePlayBackEnded);
    player.on(MediaPlayer.events.QUALITY_CHANGE_REQUESTED, eQualityChange);
    player.on(MediaPlayer.events.TRACK_CHANGE_RENDERED, eTrackChange);

    return () => {
      player.off(MediaPlayer.events.PLAYBACK_PAUSED, ePlayBackPaused);
      player.off(MediaPlayer.events.PLAYBACK_PLAYING, ePlayBackPlaying);
      player.off(MediaPlayer.events.PLAYBACK_WAITING, ePlayBackWaiting);
      player.off(
        MediaPlayer.events.PLAYBACK_TIME_UPDATED,
        ePlayBackTimeUpdated
      );
      player.off(MediaPlayer.events.PLAYBACK_NOT_ALLOWED, ePlayBackNotAllowed);
      player.off(MediaPlayer.events.PLAYBACK_ENDED, ePlayBackEnded);
      player.off(MediaPlayer.events.QUALITY_CHANGE_REQUESTED, eQualityChange);
      player.off(MediaPlayer.events.TRACK_CHANGE_RENDERED, eTrackChange);
    };
  }, [
    ePlayBackEnded,
    ePlayBackNotAllowed,
    ePlayBackPaused,
    ePlayBackPlaying,
    ePlayBackTimeUpdated,
    ePlayBackWaiting,
    eQualityChange,
    eTrackChange,
    player,
  ]);

  return null;
}

export default VideoEvents;
