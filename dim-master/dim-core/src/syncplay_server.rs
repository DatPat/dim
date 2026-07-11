//! Syncplay-compatible TCP server.
//!
//! Listens on a configurable port and speaks the Syncplay JSON-over-TCP
//! protocol (newline-delimited JSON). Each connection maps to a
//! `SyncplayClient` participant in the unified `SyncEngine`.

use std::time::Duration;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::sync_engine::{
    now_ms, ParticipantId, ParticipantKind, SyncEngine, SyncParticipant,
};
use crate::syncplay_proto::{self, ClientMessage};

const SYNCPLAY_VERSION: &str = "1.7.3";

/// Kill connections that have been silent for this long. Syncplay clients
/// send a State roughly every second, so this only fires for half-open TCP
/// connections that would otherwise leak ghost participants.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Per-connection Syncplay protocol state (the `ignoringOnTheFly` handshake
/// and latency bookkeeping). Lives entirely inside the connection task.
#[derive(Default)]
struct ConnProto {
    /// Counter attached to server-initiated State messages. The client echoes
    /// it back; until it does, its own (stale) playstate reports are ignored.
    server_iotf: u64,
    awaiting_server_echo: bool,
    /// The client's `clientLatencyCalculation` timestamp, echoed back in our
    /// replies so the client can compute its RTT.
    client_latency_calculation: Option<f64>,
    /// Server-side RTT estimate, computed when the client echoes our
    /// `latencyCalculation` timestamp.
    server_rtt: f64,
}

/// Inject the `ignoringOnTheFly` server counter into an outgoing State
/// message (a server-initiated state change the client must obey).
fn tag_state_with_iotf(message: &str, proto: &mut ConnProto) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(message) else {
        return message.to_string();
    };

    if let Some(state) = v.get_mut("State") {
        proto.server_iotf += 1;
        proto.awaiting_server_echo = true;
        state["ignoringOnTheFly"] = serde_json::json!({ "server": proto.server_iotf });
        return v.to_string();
    }

    message.to_string()
}

async fn write_line(
    writer: &mut OwnedWriteHalf,
    line: &str,
) -> Result<(), std::io::Error> {
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\r\n").await
}

/// Run the Syncplay TCP server. Call this from main.rs.
pub async fn run_syncplay_server(
    bind_addr: std::net::SocketAddr,
    engine: SyncEngine,
    motd: String,
    password: String,
) {
    let listener = match TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind Syncplay server on {}: {}", bind_addr, e);
            return;
        }
    };

    tracing::info!(%bind_addr, "Syncplay server listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("Syncplay accept error: {}", e);
                continue;
            }
        };

        tracing::debug!(%peer, "Syncplay connection accepted");

        let engine = engine.clone();
        let motd = motd.clone();
        let password = password.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_syncplay_connection(stream, engine, motd, password).await {
                tracing::debug!(%peer, "Syncplay connection ended: {}", e);
            }
        });
    }
}

/// Check the Hello password against the configured server password. Official
/// Syncplay clients send an MD5 hex digest; accept plaintext too for
/// simpler custom clients.
fn server_password_ok(configured: &str, supplied: Option<&str>) -> bool {
    if configured.is_empty() {
        return true;
    }
    let Some(supplied) = supplied else {
        return false;
    };
    if supplied == configured {
        return true;
    }
    let digest = format!("{:x}", md5::compute(configured.as_bytes()));
    supplied.eq_ignore_ascii_case(&digest)
}

async fn handle_syncplay_connection(
    stream: TcpStream,
    engine: SyncEngine,
    motd: String,
    server_password: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // --- Handshake phase ---
    // Modern clients probe with {"TLS": {"startTLS": "send"}} before Hello
    // and wait for an answer; reply that we don't do TLS so they proceed.
    let hello = loop {
        let line = match tokio::time::timeout(IDLE_TIMEOUT, lines.next_line()).await {
            Ok(l) => match l? {
                Some(l) => l,
                None => return Ok(()),
            },
            Err(_) => return Ok(()), // handshake timeout
        };

        if line.contains("\"TLS\"") || line.contains("startTLS") {
            write_line(&mut writer, &serde_json::json!({"TLS": {"startTLS": "false"}}).to_string())
                .await?;
            continue;
        }

        if let Some(ClientMessage::Hello(h)) = ClientMessage::parse(&line) {
            break h;
        }
        // Ignore other non-Hello messages during handshake
    };

    if !server_password_ok(&server_password, hello.password.as_deref()) {
        write_line(&mut writer, &syncplay_proto::server_error("Wrong password")).await?;
        return Ok(());
    }

    let username = hello.username.unwrap_or_else(|| "Anonymous".to_string());
    let room_name = hello
        .room
        .as_ref()
        .map(|r| r.name().to_string())
        .unwrap_or_else(|| "default".to_string());
    let password = hello.password.as_deref();

    // Allocate a connection ID and register in engine
    let conn_id = engine.allocate_conn_id().await;
    let participant = SyncParticipant {
        id: ParticipantId::Syncplay(conn_id),
        display_name: username.clone(),
        kind: ParticipantKind::SyncplayClient { conn_id },
        is_host: false,
        is_ready: false,
        is_buffering: false,
        latency_ms: 0,
        file: None,
    };

    // Set up the outbound message channel BEFORE joining so no notification
    // window is missed.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    engine.register_syncplay_conn(conn_id, tx).await;

    // Join or create room by name
    let (room_code, info, join_notifs) =
        match engine.join_room_by_name(participant, &room_name, password).await {
            Ok(x) => x,
            Err(e) => {
                engine.unregister_syncplay_conn(conn_id).await;
                write_line(&mut writer, &syncplay_proto::server_error(e)).await?;
                return Ok(());
            }
        };

    engine.dispatch(join_notifs).await;

    // The engine may have uniquified the username on collision — use the
    // final name everywhere (the protocol keys user state by name).
    let synthetic_uid = -(conn_id as i64);
    let username = info
        .participants
        .iter()
        .find(|p| p.user_id == synthetic_uid)
        .map(|p| p.username.clone())
        .unwrap_or(username);

    let mut proto = ConnProto::default();

    // Send Hello response
    let hello_resp = syncplay_proto::server_hello(&username, &room_name, SYNCPLAY_VERSION, &motd);
    write_line(&mut writer, &hello_resp).await?;

    // Send the initial authoritative state with doSeek so the client jumps
    // to the room position, tagged with an IOTF counter so its stale
    // position reports are ignored until it acknowledges.
    {
        let paused = info.playback_state == "paused";
        let state_msg = serde_json::json!({
            "State": {
                "playstate": {
                    "position": info.playback_position,
                    "paused": paused,
                    "doSeek": true
                },
                "ping": {
                    "latencyCalculation": now_ms() as f64 / 1000.0,
                    "serverRtt": 0
                }
            }
        });
        let tagged = tag_state_with_iotf(&state_msg.to_string(), &mut proto);
        write_line(&mut writer, &tagged).await?;
    }

    // Send the initial user list so the client knows who's in the room.
    if let Some(list) = engine.syncplay_list(&room_code).await {
        write_line(&mut writer, &list).await?;
    }

    // --- Message loop ---
    let pid = ParticipantId::Syncplay(conn_id);
    let code = room_code.clone();
    let mut last_inbound = Instant::now();

    loop {
        let idle_deadline = tokio::time::sleep_until((last_inbound + IDLE_TIMEOUT).into());

        tokio::select! {
            biased;

            // Outbound messages from engine
            msg = rx.recv() => {
                match msg {
                    Some(m) => {
                        // Server-initiated State changes must carry an
                        // ignoringOnTheFly counter so the client's in-flight
                        // (stale) reports don't fight the new state.
                        let m = if m.starts_with("{\"State\"") {
                            tag_state_with_iotf(&m, &mut proto)
                        } else {
                            m
                        };
                        write_line(&mut writer, &m).await?;
                    }
                    None => break, // channel closed (room destroyed)
                }
            }

            // Inbound messages from client
            line_result = lines.next_line() => {
                let line = match line_result? {
                    Some(l) => l,
                    None => break, // connection closed
                };
                last_inbound = Instant::now();

                let msg = match ClientMessage::parse(&line) {
                    Some(m) => m,
                    None => continue, // unparseable, skip
                };

                match msg {
                    ClientMessage::State(state) => {
                        // --- Latency bookkeeping ---
                        let mut latency_ms = 0u32;
                        if let Some(ping) = state.ping.as_ref() {
                            if let Some(rtt) = ping.client_rtt {
                                latency_ms = (rtt.max(0.0) * 1000.0) as u32;
                            }
                            proto.client_latency_calculation = ping.client_latency_calculation;
                            // The client echoes our latencyCalculation
                            // timestamp; use it to estimate server-side RTT.
                            if let Some(echo) = ping.latency_calculation {
                                let now_s = now_ms() as f64 / 1000.0;
                                let rtt = now_s - echo;
                                if rtt >= 0.0 && rtt < 30.0 {
                                    proto.server_rtt = rtt;
                                }
                            }
                        }

                        // --- ignoringOnTheFly handshake ---
                        let mut client_iotf: Option<u64> = None;
                        if let Some(iotf) = state.ignoring_on_the_fly.as_ref() {
                            if let Some(echo) = iotf.get("server").and_then(|v| v.as_u64()) {
                                if echo >= proto.server_iotf {
                                    proto.awaiting_server_echo = false;
                                }
                            }
                            client_iotf = iotf.get("client").and_then(|v| v.as_u64());
                        }

                        // A playstate is only meaningful if the client isn't
                        // still reacting to a state we pushed. A client-side
                        // IOTF counter marks a deliberate user action, which
                        // is always processed.
                        let playstate = state.playstate.as_ref().and_then(|ps| {
                            let position = ps.position?;
                            let paused = ps.paused?;
                            if proto.awaiting_server_echo && client_iotf.is_none() {
                                return None;
                            }
                            Some((position, paused, ps.do_seek.unwrap_or(false)))
                        });

                        let Ok((snapshot, notifs)) = engine
                            .report_state(pid, &code, playstate, latency_ms)
                            .await
                        else {
                            break; // room is gone
                        };

                        engine.dispatch(notifs).await;

                        // --- Personal reply (position keepalive + ping echo) ---
                        let mut ping = serde_json::json!({
                            "latencyCalculation": now_ms() as f64 / 1000.0,
                            "serverRtt": proto.server_rtt
                        });
                        if let Some(clc) = proto.client_latency_calculation {
                            ping["clientLatencyCalculation"] = serde_json::json!(clc);
                        }

                        let mut reply = serde_json::json!({
                            "State": {
                                "playstate": {
                                    "position": snapshot.position,
                                    "paused": snapshot.paused,
                                    "doSeek": false
                                },
                                "ping": ping
                            }
                        });
                        if let Some(c) = client_iotf {
                            reply["State"]["ignoringOnTheFly"] =
                                serde_json::json!({ "client": c });
                        }
                        write_line(&mut writer, &reply.to_string()).await?;
                    }

                    ClientMessage::Chat(chat) => {
                        if let Some(text) = chat.message {
                            if let Ok(notifs) = engine.add_chat(
                                pid, &code, &username, text,
                            ).await {
                                engine.dispatch(notifs).await;
                            }
                        }
                    }

                    ClientMessage::Set(val) => {
                        // Handle file info updates
                        if let Some(file) = val.get("file") {
                            let pf = crate::sync_engine::ParticipantFile {
                                name: file.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                duration: file.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0),
                                size: file.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                                dim_file_id: file.get("dimFileId").and_then(|v| v.as_i64()).unwrap_or(0),
                            };
                            if let Ok(notifs) = engine.set_file(pid, &code, pf).await {
                                engine.dispatch(notifs).await;
                            }
                        }

                        // Handle readiness updates. Official clients send
                        // {"Set": {"ready": {"isReady": bool, "manuallyInitiated": bool}}}.
                        if let Some(ready) = val.get("ready") {
                            let is_ready = ready
                                .get("isReady")
                                .and_then(|v| v.as_bool())
                                // Tolerate the legacy {"<username>": bool} shape.
                                .or_else(|| ready.get(&username).and_then(|v| v.as_bool()));

                            if let Some(r) = is_ready {
                                if let Ok(notifs) = engine.set_ready(pid, &code, r).await {
                                    engine.dispatch(notifs).await;
                                }
                            }
                        }
                    }

                    ClientMessage::List(_) => {
                        if let Some(list) = engine.syncplay_list(&code).await {
                            write_line(&mut writer, &list).await?;
                        }
                    }

                    ClientMessage::Hello(_) => {
                        // Already handshaked, ignore
                    }
                }
            }

            // Half-open connection guard: no inbound traffic for too long.
            _ = idle_deadline => {
                tracing::debug!(conn_id, "Syncplay connection idle timeout");
                break;
            }
        }
    }

    // --- Cleanup ---
    engine.unregister_syncplay_conn(conn_id).await;
    if let Some(notifs) = engine.handle_disconnect(pid).await {
        engine.dispatch(notifs).await;
    }

    Ok(())
}
