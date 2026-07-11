FROM node:22-bookworm AS web
WORKDIR /ui
COPY dim-master/ui/package*.json ./
RUN npm ci
COPY dim-master/ui ./
ENV NODE_OPTIONS=--openssl-legacy-provider
RUN npm run build

FROM rust:bookworm AS builder
ARG DEBIAN_FRONTEND=noninteractive
ARG GIT_TAG=unknown
ARG GIT_SHA=unknown
ENV GIT_TAG=${GIT_TAG}
ENV GIT_SHA=${GIT_SHA}
RUN apt-get update && apt-get install -y \
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY nightfall-master ./nightfall-master
COPY dim-master ./dim-master
COPY --from=web /ui/build dim-master/ui/build
RUN cargo build --release --manifest-path dim-master/Cargo.toml

FROM debian:trixie-slim
ENV RUST_BACKTRACE=full
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    ffmpeg \
    libfontconfig1 \
    libfribidi0 \
    libharfbuzz0b \
    libsqlite3-0 \
    libtheora0 \
    libva-drm2 \
    libva2 \
    libvorbis0a \
    libvorbisenc2 \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /opt/dim/utils && \
    ln -s /usr/bin/ffmpeg /opt/dim/utils/ffmpeg && \
    ln -s /usr/bin/ffprobe /opt/dim/utils/ffprobe

COPY --from=builder /build/dim-master/target/release/dim /opt/dim/dim

EXPOSE 8000
VOLUME ["/opt/dim/config"]

ENV RUST_LOG=info
WORKDIR /opt/dim
CMD ["./dim"]
