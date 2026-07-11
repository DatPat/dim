//! Serde types for the Syncplay JSON-over-TCP protocol.
//!
//! Syncplay uses newline-delimited JSON over a plain TCP connection.
//! Each message is a single JSON object terminated by `\r\n`.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Client → Server messages
// ---------------------------------------------------------------------------

/// Top-level client message (exactly one key present).
#[derive(Debug, Deserialize)]
pub enum ClientMessage {
    Hello(ClientHello),
    State(ClientState),
    Set(serde_json::Value),
    List(serde_json::Value),
    Chat(ClientChat),
}

impl ClientMessage {
    /// Parse a JSON line into a ClientMessage.
    pub fn parse(line: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let obj = v.as_object()?;

        if let Some(hello) = obj.get("Hello") {
            let h: ClientHello = serde_json::from_value(hello.clone()).ok()?;
            return Some(ClientMessage::Hello(h));
        }
        if let Some(state) = obj.get("State") {
            let s: ClientState = serde_json::from_value(state.clone()).ok()?;
            return Some(ClientMessage::State(s));
        }
        if let Some(chat) = obj.get("Chat") {
            // Official clients send {"Chat": "text"}; some send
            // {"Chat": {"message": "text"}} — accept both.
            let message = if let Some(s) = chat.as_str() {
                Some(s.to_string())
            } else {
                chat.get("message")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            return Some(ClientMessage::Chat(ClientChat { message }));
        }
        if let Some(set) = obj.get("Set") {
            return Some(ClientMessage::Set(set.clone()));
        }
        if let Some(list) = obj.get("List") {
            return Some(ClientMessage::List(list.clone()));
        }

        None
    }
}

#[derive(Debug, Deserialize)]
pub struct ClientHello {
    pub username: Option<String>,
    pub room: Option<RoomSpec>,
    pub version: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub features: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RoomSpec {
    Named { name: String },
    Simple(String),
}

impl RoomSpec {
    pub fn name(&self) -> &str {
        match self {
            RoomSpec::Named { name } => name,
            RoomSpec::Simple(s) => s,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ClientState {
    #[serde(default)]
    pub playstate: Option<ClientPlaystate>,
    #[serde(default)]
    pub ping: Option<ClientPing>,
    #[serde(default, rename = "ignoringOnTheFly")]
    pub ignoring_on_the_fly: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ClientPlaystate {
    #[serde(default)]
    pub position: Option<f64>,
    #[serde(default)]
    pub paused: Option<bool>,
    #[serde(default, rename = "doSeek")]
    pub do_seek: Option<bool>,
    #[serde(default, rename = "setBy")]
    pub set_by: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClientPing {
    #[serde(default, rename = "clientRtt")]
    pub client_rtt: Option<f64>,
    #[serde(default, rename = "clientLatencyCalculation")]
    pub client_latency_calculation: Option<f64>,
    /// Echo of the server's `latencyCalculation` timestamp — used to compute
    /// the server-side RTT estimate.
    #[serde(default, rename = "latencyCalculation")]
    pub latency_calculation: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct ClientChat {
    #[serde(default)]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Server → Client messages (constructed as JSON values)
// ---------------------------------------------------------------------------

/// Build the server Hello response.
pub fn server_hello(username: &str, room_name: &str, version: &str, motd: &str) -> String {
    let msg = serde_json::json!({
        "Hello": {
            "username": username,
            "room": { "name": room_name },
            "version": version,
            "motd": if motd.is_empty() { None } else { Some(motd) },
            "features": {
                "managedRooms": true,
                "chat": true,
                "readiness": true
            }
        }
    });
    msg.to_string()
}

/// Build a server Error response.
pub fn server_error(message: &str) -> String {
    let msg = serde_json::json!({
        "Error": {
            "message": message
        }
    });
    msg.to_string()
}

// State, user list, and ready messages are constructed in sync_engine.rs.
