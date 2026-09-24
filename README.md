# Dim — a maintained fork

A self-hosted media manager built with Rust and React, based on
[Dusk-Labs/dim](https://github.com/Dusk-Labs/dim). Upstream development has
been dormant for a long time; this fork has been running in production
(on both x86 and ARM hardware) and has accumulated a large number of fixes
and features on top of the last upstream snapshot.

Ready-built multi-arch images (amd64 + arm64) are on Docker Hub:

```sh
docker run -d \
  -p 8000:8000 \
  -v /path/to/config:/opt/dim/config \
  -v /path/to/media:/media \
  trashcorpinc/dim:latest
```

## Layout

| Path | Contents |
|------|----------|
| `dim-master/` | The dim server (Rust workspace) and web UI (React) |
| `nightfall-master/` | The transcoding/streaming engine (used as a path dependency) |
| `Dockerfile.allfeatures` | Production image: dim with all hardware backends + ffmpeg built from source |
| `build-*.sh` | Convenience build scripts |

This repository is a source snapshot fork — it does not carry upstream git
history.

## What's different from upstream

### Playback & streaming

- **VP9 direct play** — VP9 sources are transmuxed into fMP4 (`vp09` sample
  entries) instead of being force-transcoded.
- **Direct-play timing validation** — files whose keyframes cannot match the
  fixed DASH segment timeline automatically use transcoding, preventing
  accumulated timing drift. Packet probes are cached by file size and
  modification time; a first probe can take up to 30 seconds, after which
  playback falls back to transcoding if timing could not be verified.
- **H.266/VVC support** via software transcode fallback.
- **Player freeze fixes**: an error boundary around the player (a render crash
  no longer blanks the whole app), a stall guard, dash.js gap-jumping, and a
  fix for a dash.js `destroy()` crash that unmounted the entire UI.
- **Lenient ffprobe parsing** — files with exotic streams (e.g. fansub
  releases with font attachments) no longer fail to index or play.
- A sweep of session, manifest, quality-selection, audio and subtitle fixes.
- **Watched threshold**: quitting during the outro/credits (past 90%) counts
  as watched — banners, resume prompts and next-episode logic all agree. The
  player flushes the exact playback position when closed.
- **End-of-episode UX**: a Next Episode overlay during credits, a *Return to
  Show* button on season/series finales, and a close button that takes you to
  the page of what you just watched instead of replaying browser history.

### Hardware transcoding

- Backends: **VAAPI, CUDA/NVENC, Intel QSV, AMD AMF, and V4L2 stateful M2M**
  (e.g. Rockchip/Amlogic/Arm Mali "Linlon" video blocks — dim runs hardware
  transcodes on small ARM boxes).
- H.264/H.265/AV1 encoding on QSV, CUDA, VAAPI and AMF; H.264/H.265 on V4L2.
- A hardware-acceleration setting (Off / Auto / per-backend) plus a decode
  method override, device detection, input-format validation before a
  hardware path is chosen, and automatic fallback to software when a
  hardware transcode fails mid-flight.

### Scanner & metadata

- **Multiple metadata providers**: TMDB, TVMaze and AniList with per-library
  provider selection and chained fallback (namespaced external ids).
- **NFO ingestion** — existing `.nfo` files pin exact matches.
- **Smart scan** — directory-context-aware matching: new files in a folder of
  already-matched episodes reuse that show instead of re-guessing from the
  filename.
- Extensive matcher hardening: filename precleaning, season-folder overrides,
  similarity tie-breaks, anime absolute-numbering mapping, alternative-title
  rescue, and deduplication via external ids.
- Rematch/automatch endpoints and a per-file diagnose endpoint for debugging
  why something matched (or didn't).

### Watch Together

- **Built-in watch-together rooms**: synchronized playback with host-only or
  egalitarian control, room join links, and chat.
- **Syncplay protocol support** — a Syncplay-compatible server/proxy, so
  desktop [Syncplay](https://syncplay.pl/) clients can join sessions.

### Integrations

- **Sonarr/Radarr webhooks that actually do something**: an import event
  locates the file inside a dim library (re-rooting paths when the *arr app
  and dim see the media tree under different mount prefixes), ingests it, and
  matches it against the authoritative TMDB/TVDB/IMDB id from the payload —
  no filename guessing. Delete, rename and upgrade events keep the index in
  sync even where filesystem watching doesn't work (network mounts). 
- An in-app **notification center** with poster thumbnails that deep-link to
  the added media.

### Nightfall (transcoding engine)

- Correct fMP4 output: `moof` box rewrites are binary-patched rather than
  round-tripped through a serializer that corrupted them.
- New profiles for the additional hardware backends and VP9 transmuxing.

### Packaging

- **Multi-arch Docker images** (linux/amd64 + linux/arm64) from a single
  `Dockerfile.allfeatures`.
- **ffmpeg 8.0.1 built from source** inside the image: upstream ffmpeg with
  libvpl and libsvtav1 on amd64; on arm64 a build with patches for Arm Linlon
  MVX V4L2 encoding.

## Building

Docker (recommended):

```sh
docker buildx build --platform linux/amd64,linux/arm64 \
  -f Dockerfile.allfeatures -t dim:latest .
```

From source: build the UI first, then the server (the workspace picks up
`nightfall-master` as a path dependency).

```sh
cd dim-master/ui && npm install && npm run build
cd .. && cargo build --release
```

## Configuring Sonarr/Radarr

Set `webhook_api_key` in dim's `config.toml`, then add a webhook connection
in Sonarr/Radarr pointing at:

```
http://<dim-host>:8000/api/v1/webhook/sonarr?apikey=<key>   (Sonarr)
http://<dim-host>:8000/api/v1/webhook/radarr?apikey=<key>   (Radarr)
```

Enable it for *On Import*, *On Upgrade*, *On Rename* and *On Delete* events.

## License

AGPL-3.0, same as upstream. See `dim-master/LICENSE.md`.
