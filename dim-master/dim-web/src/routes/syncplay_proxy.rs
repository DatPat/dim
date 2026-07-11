//! WebSocket-to-TCP proxy for connecting to external Syncplay servers.
//!
//! Browsers can't make raw TCP connections, so this endpoint upgrades
//! an HTTP request to WebSocket, connects to the specified Syncplay server
//! via TCP, and relays JSON lines bidirectionally.
//!
//! Route: GET /api/v1/syncplay/proxy?host=<host>&port=<port>&token=<auth>
//!
//! The route is registered outside the cookie middleware (browsers can't set
//! headers on WebSocket upgrades), so authentication happens in-band via the
//! `token` query parameter. Without it the endpoint would be an
//! unauthenticated SSRF primitive (arbitrary TCP connects from the server).

use axum::extract::{ConnectInfo, Query, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use http::StatusCode;
use serde::Deserialize;
use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::AppState;

#[derive(Deserialize)]
pub struct ProxyParams {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub token: Option<String>,
}

pub async fn syncplay_proxy(
    ws: WebSocketUpgrade,
    Query(params): Query<ProxyParams>,
    ConnectInfo(_remote): ConnectInfo<SocketAddr>,
    State(app): State<AppState>,
) -> Response {
    // Authenticate the token in-band.
    let authed = match params.token.as_deref() {
        Some(token) => match dim_database::user::Login::verify_cookie(token.to_string()) {
            Ok(user_id) => {
                if let Ok(mut tx) = app.conn.read().begin().await {
                    dim_database::user::User::get_by_id(&mut tx, user_id)
                        .await
                        .is_ok()
                } else {
                    false
                }
            }
            Err(_) => false,
        },
        None => false,
    };

    if !authed {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response();
    }

    ws.on_upgrade(move |websocket| async move {
        if let Err(e) = handle_proxy(websocket, &params.host, params.port).await {
            tracing::warn!("Syncplay proxy error for {}:{}: {}", params.host, params.port, e);
        }
    })
}

async fn handle_proxy(
    websocket: axum::extract::ws::WebSocket,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("{}:{}", host, port);

    let (mut ws_tx, mut ws_rx) = websocket.split();

    let tcp = match TcpStream::connect(&addr).await {
        Ok(stream) => stream,
        Err(e) => {
            // Send an error message the client can display before closing
            let err_msg = serde_json::json!({
                "Error": { "message": format!("Failed to connect to {}: {}", addr, e) }
            });
            let _ = ws_tx
                .send(axum::extract::ws::Message::Text(err_msg.to_string()))
                .await;
            return Err(Box::new(e));
        }
    };

    let (tcp_reader, mut tcp_writer) = tcp.into_split();
    let mut tcp_lines = BufReader::new(tcp_reader).lines();

    // Channel so the ws_to_tcp task can hand Pong replies to the single
    // WebSocket sink owner (tcp_to_ws).
    let (pong_tx, mut pong_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    // Relay: TCP → WebSocket (also delivers Pong replies)
    let tcp_to_ws = async {
        loop {
            tokio::select! {
                line = tcp_lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    if ws_tx
                        .send(axum::extract::ws::Message::Text(line))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                pong = pong_rx.recv() => {
                    let Some(payload) = pong else { break };
                    if ws_tx
                        .send(axum::extract::ws::Message::Pong(payload))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    };

    // Relay: WebSocket → TCP
    let ws_to_tcp = async {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                axum::extract::ws::Message::Text(text) => {
                    if tcp_writer.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                    if tcp_writer.write_all(b"\r\n").await.is_err() {
                        break;
                    }
                }
                // axum does not auto-reply to pings; keepalive-enforcing
                // clients/proxies drop the connection without a Pong.
                axum::extract::ws::Message::Ping(payload) => {
                    if pong_tx.send(payload).is_err() {
                        break;
                    }
                }
                axum::extract::ws::Message::Close(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = tcp_to_ws => {},
        _ = ws_to_tcp => {},
    }

    Ok(())
}
