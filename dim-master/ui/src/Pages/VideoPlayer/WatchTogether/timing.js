// Keep server timestamps out of playback arithmetic: wall clocks on different
// machines need not agree. Track snapshot age on the client's monotonic clock.
export function stampRoom(room, requestStartedAt) {
  const receivedAt = performance.now();
  return {
    ...room,
    client_received_at: receivedAt,
    transit_seconds: Math.max(0, receivedAt - requestStartedAt) / 2000,
  };
}

export function playbackPosition(position, playing, receivedAt, transitSeconds = 0) {
  const age = receivedAt == null ? 0 : Math.max(0, performance.now() - receivedAt) / 1000;
  return Math.max(0, position + (playing ? age + transitSeconds : 0));
}
