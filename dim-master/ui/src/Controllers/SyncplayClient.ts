/**
 * SyncplayClient — speaks the Syncplay JSON-over-TCP protocol
 * through a WebSocket proxy (GET /api/v1/syncplay/proxy?host=&port=).
 *
 * Host/guest model:
 *   - Calling setFile() with markHost (default) marks us as the host.
 *   - Receiving another user's file when isHost is false triggers
 *     onFileChange so the UI can navigate and become a guest.
 *   - Hosts send their real local state; guests echo the server's
 *     authoritative state in auto-responses.
 */

interface SyncplayOptions {
  host: string;
  port: number;
  room: string;
  username: string;
  password?: string;
  /** Dim auth token — required by the proxy endpoint. */
  token?: string;
}

type StateUpdateHandler = (position: number, paused: boolean, doSeek: boolean) => void;
type ChatHandler = (username: string, message: string) => void;
type UserListHandler = (users: Record<string, any>) => void;
type FileChangeHandler = (fileId: number, fileName: string) => void;
type ErrorHandler = (message: string) => void;
type ConnectedHandler = (username: string, room: string, motd?: string) => void;
type DisconnectedHandler = () => void;

/** Pattern we embed in the filename so external Syncplay servers
 *  (which strip unknown fields) still carry the Dim file ID. */
const DIM_TAG_RE = /\[dim:(\d+)\]$/;

interface CachedFile {
  name: string;
  duration: number;
  size: number;
  dimFileId?: number;
}

class SyncplayClient {
  private ws: WebSocket | null = null;
  private options: SyncplayOptions;
  private stateInterval: ReturnType<typeof setInterval> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalDisconnect: boolean = false;

  // Last known local playback state, used to auto-respond to server pings.
  // `lastPositionSetAt` lets auto-responses extrapolate the position while
  // playing instead of echoing a stale snapshot.
  private lastPosition: number = 0;
  private lastPaused: boolean = true;
  private lastPositionSetAt: number = Date.now() / 1000;
  private lastServerLatencyCalc: number = 0;
  private lastClientRtt: number = 0;
  private serverIgnoringOnTheFly: number = 0;

  // Client-side ignoringOnTheFly: incremented when WE initiate a change.
  // Until the server echoes the counter back, incoming server playstates are
  // stale (they predate our change) and must be ignored.
  private clientIgnoringOnTheFly: number = 0;
  private awaitingClientEcho: boolean = false;
  private pendingClientIotf: number | null = null;

  /** Cached so a reconnect can re-announce the file (the server forgets it). */
  private lastFile: CachedFile | null = null;

  /** True when we're the host. Hosts send real local state; guests echo
   *  server state and follow file changes. */
  isHost: boolean = false;

  /** @deprecated kept for backwards compatibility with older callers. */
  get hasSetFile(): boolean {
    return this.isHost;
  }

  onStateUpdate: StateUpdateHandler | null = null;
  onChat: ChatHandler | null = null;
  onUserList: UserListHandler | null = null;
  onFileChange: FileChangeHandler | null = null;
  onError: ErrorHandler | null = null;
  onConnected: ConnectedHandler | null = null;
  onDisconnected: DisconnectedHandler | null = null;

  constructor(options: SyncplayOptions) {
    this.options = options;
  }

  connect() {
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const token = this.options.token ? `&token=${encodeURIComponent(this.options.token)}` : "";
    const wsUrl = `${proto}//${window.location.host}/api/v1/syncplay/proxy?host=${encodeURIComponent(this.options.host)}&port=${this.options.port}${token}`;

    this.ws = new WebSocket(wsUrl);

    this.ws.onopen = () => {
      const hello: any = {
        Hello: {
          username: this.options.username,
          room: { name: this.options.room },
          version: "1.7.3",
          realversion: "1.7.3",
          features: {
            sharedPlaylists: false,
            chat: true,
            featureList: true,
            readiness: true,
            managedRooms: true,
          },
        },
      };
      if (this.options.password) {
        hello.Hello.password = this.options.password;
      }
      this.ws!.send(JSON.stringify(hello));
    };

    this.ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        this.handleMessage(data);
      } catch {
        // ignore unparseable
      }
    };

    this.ws.onclose = () => {
      if (!this.intentionalDisconnect) {
        // Auto-reconnect after 2s
        this.reconnectTimer = setTimeout(() => {
          this.connect();
        }, 2000);
      } else {
        this.cleanup();
      }
      this.onDisconnected?.();
    };

    this.ws.onerror = () => {
      this.onError?.("Connection error");
    };
  }

  private handleMessage(data: any) {
    if (data.Hello) {
      const h = data.Hello;
      this.onConnected?.(h.username, h.room?.name || "", h.motd);

      // Reconnect: the server no longer knows our file — re-announce it and
      // our current state so the session resumes seamlessly.
      if (this.lastFile) {
        this.sendFileMessage(this.lastFile);
      }
      return;
    }

    if (data.State) {
      const serverPing = data.State.ping;
      if (serverPing) {
        // RTT = our clock now minus OUR timestamp echoed back by the server.
        // (latencyCalculation is the server's clock — comparing it to ours
        // measures clock skew, not latency.)
        if (serverPing.clientLatencyCalculation) {
          this.lastClientRtt = Math.max(
            0,
            Date.now() / 1000 - serverPing.clientLatencyCalculation
          );
        }
        if (serverPing.latencyCalculation) {
          this.lastServerLatencyCalc = serverPing.latencyCalculation;
        }
      }

      const iotf = data.State.ignoringOnTheFly;
      if (iotf?.server != null) {
        this.serverIgnoringOnTheFly = iotf.server;
      }
      if (iotf?.client != null && iotf.client >= this.clientIgnoringOnTheFly) {
        // Server acknowledged our change; its states are fresh again.
        this.awaitingClientEcho = false;
      }

      const ps = data.State.playstate;
      // While we're awaiting the echo of our own change, incoming server
      // playstates predate it — applying them would undo the user's action.
      if (ps && !this.awaitingClientEcho) {
        const pos = ps.position ?? 0;
        const paused = ps.paused ?? true;
        const doSeek = ps.doSeek ?? false;

        // Guests always adopt the server's authoritative state so their
        // auto-response echoes it correctly. Hosts keep their own local
        // values set by reportState().
        if (!this.isHost) {
          this.lastPosition = pos;
          this.lastPaused = paused;
          this.lastPositionSetAt = Date.now() / 1000;
        }

        this.onStateUpdate?.(pos, paused, doSeek);
      }

      this.sendState(false);
      return;
    }

    if (data.Chat) {
      this.onChat?.(data.Chat.username || "", data.Chat.message || "");
      return;
    }

    if (data.Set) {
      if (data.Set.user) {
        // Parse user list. syncplay.pl (and Dim's server) send per-user
        // messages: {"user": {"Username": {"room": {...}, "file": {...}}}}
        // Older Dim builds sent room-grouped {"user": {"roomName": {...}}}.
        // Detect format by checking if values have "room"/"file"/"event" keys.
        const allUsers: Record<string, any> = {};
        for (const [key, val] of Object.entries(data.Set.user)) {
          if (val && typeof val === "object" && ("room" in val || "file" in val || "event" in val)) {
            // Format 1 (syncplay.pl): key IS the username
            allUsers[key] = val;
          } else if (val && typeof val === "object") {
            // Format 2 (legacy Dim server): key is room name, val contains users
            Object.assign(allUsers, val);
          }
        }
        this.onUserList?.(allUsers);
        this.maybeFollowFile(allUsers);
      }
      return;
    }

    if (data.List) {
      // Full user list, keyed by room name.
      const allUsers: Record<string, any> = {};
      for (const val of Object.values(data.List)) {
        if (val && typeof val === "object") {
          Object.assign(allUsers, val);
        }
      }
      this.onUserList?.(allUsers);
      this.maybeFollowFile(allUsers);
      return;
    }

    if (data.Error) {
      this.onError?.(data.Error.message || "Unknown error");
      return;
    }
  }

  private maybeFollowFile(allUsers: Record<string, any>) {
    // If we're the host, we don't follow other users' files.
    if (this.isHost) return;

    for (const [username, info] of Object.entries(allUsers)) {
      if (username === this.options.username) continue;
      const file = (info as any)?.file;
      if (!file || !file.name) continue;

      // Try the explicit dimFileId field first (Dim's own server).
      if (file.dimFileId && file.dimFileId > 0) {
        this.onFileChange?.(file.dimFileId, file.name || "");
        return;
      }

      // Parse [dim:<id>] tag from filename (external servers).
      const name: string = file.name || "";
      const m = name.match(DIM_TAG_RE);
      if (m) {
        const id = parseInt(m[1], 10);
        const cleanName = name.replace(DIM_TAG_RE, "").trim();
        this.onFileChange?.(id, cleanName);
        return;
      }
    }
  }

  private currentPosition(): number {
    if (this.lastPaused) return this.lastPosition;
    return this.lastPosition + (Date.now() / 1000 - this.lastPositionSetAt);
  }

  private sendState(userInitiated: boolean, doSeek: boolean = false) {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;

    const msg: any = {
      State: {
        playstate: {
          position: this.currentPosition(),
          paused: this.lastPaused,
          doSeek,
          setBy: userInitiated ? this.options.username : null,
        },
        ping: {
          clientRtt: this.lastClientRtt,
          clientLatencyCalculation: Date.now() / 1000,
          latencyCalculation: this.lastServerLatencyCalc,
        },
      },
    };

    const iotf: any = {};
    if (this.serverIgnoringOnTheFly > 0) {
      iotf.server = this.serverIgnoringOnTheFly;
      this.serverIgnoringOnTheFly = 0;
    }
    if (userInitiated) {
      this.clientIgnoringOnTheFly += 1;
      this.awaitingClientEcho = true;
      iotf.client = this.clientIgnoringOnTheFly;
    } else if (this.pendingClientIotf != null) {
      iotf.client = this.pendingClientIotf;
      this.pendingClientIotf = null;
    }
    if (Object.keys(iotf).length > 0) {
      msg.State.ignoringOnTheFly = iotf;
    }

    this.ws.send(JSON.stringify(msg));
  }

  /** Report the local playback state. `userInitiated` marks deliberate user
   *  actions (seek, pause toggle) that the server must adopt. */
  reportState(position: number, paused: boolean, doSeek: boolean = false) {
    const userInitiated = doSeek || paused !== this.lastPaused;
    this.lastPosition = position;
    this.lastPaused = paused;
    this.lastPositionSetAt = Date.now() / 1000;
    this.sendState(userInitiated, doSeek);
  }

  /** Tell the server what file is playing.
   *  `markHost` (default true) marks us as the session host; guests announce
   *  their file too (so peers see it) but must NOT become hosts, or they
   *  stop following the real host and echo stale state forever. */
  setFile(
    name: string,
    duration: number,
    size?: number,
    dimFileId?: number,
    markHost: boolean = true
  ) {
    if (markHost) {
      this.isHost = true;
    }

    let taggedName = name;
    if (dimFileId && dimFileId > 0) {
      taggedName = `${name} [dim:${dimFileId}]`;
    }

    this.lastFile = { name: taggedName, duration, size: size ?? 0, dimFileId };
    this.sendFileMessage(this.lastFile);
  }

  private sendFileMessage(file: CachedFile) {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;

    const msg = {
      Set: {
        file: {
          name: file.name,
          duration: file.duration,
          size: file.size,
        },
      },
    };
    this.ws.send(JSON.stringify(msg));
  }

  sendChat(message: string) {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify({ Chat: message }));
  }

  setReady(isReady: boolean) {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    // Official Syncplay shape — servers identify us by connection.
    const msg = {
      Set: {
        ready: {
          isReady,
          manuallyInitiated: true,
        },
      },
    };
    this.ws.send(JSON.stringify(msg));
  }

  disconnect() {
    this.intentionalDisconnect = true;
    this.cleanup();
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  private cleanup() {
    if (this.stateInterval) {
      clearInterval(this.stateInterval);
      this.stateInterval = null;
    }
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }
}

export default SyncplayClient;
