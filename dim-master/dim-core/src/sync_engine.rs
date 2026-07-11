use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use once_cell::sync::OnceCell;
use rand::Rng;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

const ROOM_CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const ROOM_CODE_LEN: usize = 6;
const MAX_CHAT_MESSAGES: usize = 200;

/// A controlling client's position report within this many seconds of the
/// room's computed position is treated as normal playback drift and adopted;
/// larger gaps without an explicit seek are ignored (the client converges to
/// the authoritative position instead).
const DRIFT_ADOPT_THRESHOLD_SECS: f64 = 4.0;

fn generate_room_code() -> String {
    let mut rng = rand::thread_rng();
    (0..ROOM_CODE_LEN)
        .map(|_| ROOM_CODE_CHARS[rng.gen_range(0..ROOM_CODE_CHARS.len())] as char)
        .collect()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    HostOnly,
    Egalitarian,
}

impl ControlMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ControlMode::HostOnly => "host_only",
            ControlMode::Egalitarian => "egalitarian",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackState {
    Playing,
    Paused,
}

impl PlaybackState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ParticipantKind {
    DimUser {
        user_id: i64,
        picture: Option<i64>,
    },
    SyncplayClient {
        conn_id: u64,
    },
}

/// Unique identifier for a participant (works for both Dim users and Syncplay clients).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantId {
    DimUser(i64),
    Syncplay(u64),
}

#[derive(Debug, Clone, Default)]
pub struct ParticipantFile {
    pub name: String,
    pub duration: f64,
    pub size: u64,
    pub dim_file_id: i64,
}

#[derive(Debug, Clone)]
pub struct SyncParticipant {
    pub id: ParticipantId,
    pub display_name: String,
    pub kind: ParticipantKind,
    pub is_host: bool,
    pub is_ready: bool,
    pub is_buffering: bool,
    pub latency_ms: u32,
    pub file: Option<ParticipantFile>,
}

impl SyncParticipant {
    pub fn user_id(&self) -> i64 {
        match &self.kind {
            ParticipantKind::DimUser { user_id, .. } => *user_id,
            // Syncplay clients get a synthetic negative user_id for events
            ParticipantKind::SyncplayClient { conn_id } => -(*conn_id as i64),
        }
    }

    pub fn picture(&self) -> Option<i64> {
        match &self.kind {
            ParticipantKind::DimUser { picture, .. } => *picture,
            ParticipantKind::SyncplayClient { .. } => None,
        }
    }

    pub fn client_type(&self) -> &'static str {
        match &self.kind {
            ParticipantKind::DimUser { .. } => "dim",
            ParticipantKind::SyncplayClient { .. } => "syncplay",
        }
    }

    fn to_wt_participant(&self) -> dim_events::WtParticipant {
        dim_events::WtParticipant {
            user_id: self.user_id(),
            username: self.display_name.clone(),
            picture: self.picture(),
            is_host: self.is_host,
            is_buffering: self.is_buffering,
            is_ready: self.is_ready,
            client_type: self.client_type().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub user_id: i64,
    pub username: String,
    pub text: String,
    pub timestamp_ms: u64,
}

#[derive(Debug)]
pub struct SyncRoom {
    pub code: String,
    pub name: String,
    pub password: Option<String>,
    pub control_mode: ControlMode,
    pub media_file_id: Option<i64>,
    pub media_id: Option<i64>,
    pub media_name: String,
    pub playback_state: PlaybackState,
    pub playback_position: f64,
    pub last_position_update: Instant,
    pub participants: Vec<SyncParticipant>,
    pub chat_messages: Vec<ChatMessage>,
}

impl SyncRoom {
    pub fn current_position(&self) -> f64 {
        match self.playback_state {
            PlaybackState::Playing => {
                self.playback_position + self.last_position_update.elapsed().as_secs_f64()
            }
            PlaybackState::Paused => self.playback_position,
        }
    }

    fn syncplay_conn_ids(&self) -> Vec<u64> {
        self.participants
            .iter()
            .filter_map(|p| match &p.kind {
                ParticipantKind::SyncplayClient { conn_id } => Some(*conn_id),
                _ => None,
            })
            .collect()
    }

    fn to_wt_participants(&self) -> Vec<dim_events::WtParticipant> {
        self.participants.iter().map(|p| p.to_wt_participant()).collect()
    }

    fn host_id(&self) -> Option<ParticipantId> {
        self.participants.iter().find(|p| p.is_host).map(|p| p.id)
    }

    fn can_control(&self, participant_id: ParticipantId) -> bool {
        match &self.control_mode {
            ControlMode::HostOnly => self.host_id() == Some(participant_id),
            ControlMode::Egalitarian => self.participants.iter().any(|p| p.id == participant_id),
        }
    }

    fn participant_name(&self, participant_id: ParticipantId) -> Option<String> {
        self.participants
            .iter()
            .find(|p| p.id == participant_id)
            .map(|p| p.display_name.clone())
    }
}

/// Serializable room info returned by REST endpoints.
#[derive(Serialize)]
pub struct RoomInfo {
    pub code: String,
    pub media_file_id: Option<i64>,
    pub media_id: Option<i64>,
    pub media_name: String,
    pub host_user_id: i64,
    pub playback_state: String,
    pub playback_position: f64,
    pub server_time_ms: u64,
    pub participants: Vec<dim_events::WtParticipant>,
    pub control_mode: String,
}

impl SyncRoom {
    pub fn to_info(&self) -> RoomInfo {
        let host_user_id = self
            .participants
            .iter()
            .find(|p| p.is_host)
            .map(|p| p.user_id())
            .unwrap_or(-1);

        RoomInfo {
            code: self.code.clone(),
            media_file_id: self.media_file_id,
            media_id: self.media_id,
            media_name: self.media_name.clone(),
            host_user_id,
            playback_state: self.playback_state.as_str().to_string(),
            playback_position: self.current_position(),
            server_time_ms: now_ms(),
            participants: self.to_wt_participants(),
            control_mode: self.control_mode.as_str().to_string(),
        }
    }
}

/// Snapshot of a room's authoritative playback state, returned to transports
/// that must compose per-connection replies (the Syncplay TCP server).
#[derive(Debug, Clone, Copy)]
pub struct RoomPlaybackSnapshot {
    pub position: f64,
    pub paused: bool,
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

/// Action to broadcast a message to specific WebSocket clients.
#[derive(Debug)]
pub struct WsBroadcast {
    pub targets: Vec<SocketAddr>,
    pub message: String,
}

/// A Syncplay protocol message to send to specific TCP clients.
pub struct SyncplayBroadcast {
    pub conn_ids: Vec<u64>,
    pub message: String,
}

/// Notifications produced by engine methods. Callers dispatch these (or use
/// `SyncEngine::dispatch`, which delivers both arms).
pub enum SyncNotification {
    WebSocket(WsBroadcast),
    Syncplay(SyncplayBroadcast),
}

// ---------------------------------------------------------------------------
// Event message constructors (for WebSocket / dim-events)
// ---------------------------------------------------------------------------

fn make_room_update_message(room: &SyncRoom) -> String {
    let info = room.to_info();
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherRoomUpdate {
            room_code: info.code,
            media_file_id: info.media_file_id.unwrap_or(0),
            media_name: info.media_name,
            playback_state: info.playback_state,
            playback_position: info.playback_position,
            server_time_ms: info.server_time_ms,
            participants: info.participants,
        },
    }
    .to_string()
}

fn make_sync_message(room_code: &str, action: &str, position: f64) -> String {
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherSync {
            room_code: room_code.to_string(),
            action: action.to_string(),
            position,
            server_time_ms: now_ms(),
        },
    }
    .to_string()
}

fn make_chat_message_event(
    room_code: &str,
    user_id: i64,
    username: &str,
    text: &str,
    timestamp_ms: u64,
) -> String {
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherChat {
            room_code: room_code.to_string(),
            user_id,
            username: username.to_string(),
            text: text.to_string(),
            timestamp_ms,
        },
    }
    .to_string()
}

fn make_room_destroyed_message(room_code: &str, reason: &str) -> String {
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherRoomDestroyed {
            room_code: room_code.to_string(),
            reason: reason.to_string(),
        },
    }
    .to_string()
}

fn make_buffering_message(
    room_code: &str,
    user_id: i64,
    username: &str,
    is_buffering: bool,
) -> String {
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherBuffering {
            room_code: room_code.to_string(),
            user_id,
            username: username.to_string(),
            is_buffering,
        },
    }
    .to_string()
}

fn make_ready_message(room_code: &str, user_id: i64, username: &str, is_ready: bool) -> String {
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherReady {
            room_code: room_code.to_string(),
            user_id,
            username: username.to_string(),
            is_ready,
        },
    }
    .to_string()
}

fn make_mode_changed_message(room_code: &str, control_mode: &str) -> String {
    dim_events::Message {
        id: -1,
        event_type: dim_events::PushEventType::EventWatchTogetherModeChanged {
            room_code: room_code.to_string(),
            control_mode: control_mode.to_string(),
        },
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Syncplay protocol message constructors
// ---------------------------------------------------------------------------

/// Build a Syncplay `State` message carrying the room's authoritative state.
///
/// `do_seek: true` tells clients to hard-jump to the position instead of
/// slewing towards it — required for seeks to actually propagate.
pub fn make_syncplay_state_message(room: &SyncRoom, do_seek: bool, set_by: Option<&str>) -> String {
    let position = room.current_position();
    let paused = room.playback_state == PlaybackState::Paused;

    let state = serde_json::json!({
        "State": {
            "playstate": {
                "position": position,
                "paused": paused,
                "doSeek": do_seek,
                "setBy": set_by
            },
            "ping": {
                "latencyCalculation": now_ms() as f64 / 1000.0,
                "serverRtt": 0
            }
        }
    });
    // Syncplay uses newline-delimited JSON
    state.to_string()
}

fn make_syncplay_chat_message(username: &str, text: &str) -> String {
    let msg = serde_json::json!({
        "Chat": {
            "username": username,
            "message": text
        }
    });
    msg.to_string()
}

fn participant_file_json(p: &SyncParticipant) -> serde_json::Value {
    match &p.file {
        Some(f) => serde_json::json!({
            "name": f.name,
            "duration": f.duration,
            "size": f.size,
            "dimFileId": f.dim_file_id
        }),
        None => serde_json::json!({}),
    }
}

/// Build a Syncplay `Set.user` event for one user, in the shape official
/// clients expect: `{"Set":{"user":{"<name>":{"room":{"name":..},"event":{..},"file":{..}}}}}`.
fn make_syncplay_user_event(room: &SyncRoom, p: &SyncParticipant, event: Option<&str>) -> String {
    let mut user_obj = serde_json::Map::new();
    user_obj.insert(
        "room".into(),
        serde_json::json!({ "name": room.name }),
    );
    if let Some(ev) = event {
        user_obj.insert("event".into(), serde_json::json!({ ev: true }));
    }
    user_obj.insert("file".into(), participant_file_json(p));

    let msg = serde_json::json!({
        "Set": {
            "user": {
                &p.display_name: serde_json::Value::Object(user_obj)
            }
        }
    });
    msg.to_string()
}

/// Build a Syncplay `List` response: the full user list of the room.
pub fn make_syncplay_list(room: &SyncRoom) -> String {
    let mut users = serde_json::Map::new();
    for p in &room.participants {
        users.insert(
            p.display_name.clone(),
            serde_json::json!({
                "position": room.current_position(),
                "file": participant_file_json(p),
                "controller": p.is_host,
                "isReady": p.is_ready,
                "features": {}
            }),
        );
    }

    let mut room_map = serde_json::Map::new();
    room_map.insert(room.name.clone(), serde_json::Value::Object(users));

    serde_json::json!({ "List": room_map }).to_string()
}

/// Build a Syncplay `Set.ready` notification in the official shape.
fn make_syncplay_ready(username: &str, is_ready: bool) -> String {
    serde_json::json!({
        "Set": {
            "ready": {
                "username": username,
                "isReady": is_ready,
                "manuallyInitiated": true
            }
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Engine state
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct EngineState {
    rooms: HashMap<String, SyncRoom>,
    /// Maps Dim user_id to their WebSocket SocketAddr for targeted messaging.
    user_addrs: HashMap<i64, SocketAddr>,
    /// Maps Syncplay room name → Dim room code.
    room_name_aliases: HashMap<String, String>,
    /// Counter for generating unique Syncplay connection IDs.
    next_conn_id: u64,
    /// Outbound message channels for connected Syncplay TCP clients.
    syncplay_conns: HashMap<u64, mpsc::UnboundedSender<String>>,
}

/// Build a WS notification for the room's Dim participants.
fn ws_broadcast_to_room(
    user_addrs: &HashMap<i64, SocketAddr>,
    room: &SyncRoom,
    ws_message: &str,
    exclude: Option<ParticipantId>,
) -> Vec<SyncNotification> {
    let ws_addrs: Vec<SocketAddr> = room
        .participants
        .iter()
        .filter(|p| {
            matches!(p.kind, ParticipantKind::DimUser { .. })
                && exclude.map_or(true, |ex| p.id != ex)
        })
        .filter_map(|p| user_addrs.get(&p.user_id()).copied())
        .collect();

    if ws_addrs.is_empty() {
        return Vec::new();
    }

    vec![SyncNotification::WebSocket(WsBroadcast {
        targets: ws_addrs,
        message: ws_message.to_string(),
    })]
}

/// Build a Syncplay notification for the room's Syncplay participants.
fn sp_broadcast_to_room(
    room: &SyncRoom,
    message: String,
    exclude: Option<ParticipantId>,
) -> Vec<SyncNotification> {
    let conn_ids: Vec<u64> = room
        .participants
        .iter()
        .filter(|p| exclude.map_or(true, |ex| p.id != ex))
        .filter_map(|p| match &p.kind {
            ParticipantKind::SyncplayClient { conn_id } => Some(*conn_id),
            _ => None,
        })
        .collect();

    if conn_ids.is_empty() {
        return Vec::new();
    }

    vec![SyncNotification::Syncplay(SyncplayBroadcast {
        conn_ids,
        message,
    })]
}

/// Check a supplied password against the room password. Dim web clients send
/// the password in plaintext; Syncplay clients send an MD5 hex digest of it.
fn password_matches(supplied: &str, expected: &str) -> bool {
    if supplied == expected {
        return true;
    }
    let digest = format!("{:x}", md5::compute(expected.as_bytes()));
    supplied.eq_ignore_ascii_case(&digest)
}

// ---------------------------------------------------------------------------
// SyncEngine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SyncEngine {
    state: Arc<RwLock<EngineState>>,
    /// Hook installed by the web server so the engine can deliver WS
    /// broadcasts from any transport (e.g. actions by Syncplay clients).
    ws_tx: Arc<OnceCell<mpsc::UnboundedSender<WsBroadcast>>>,
}

impl Default for SyncEngine {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(EngineState::default())),
            ws_tx: Arc::new(OnceCell::new()),
        }
    }
}

impl Clone for SyncEngine {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            ws_tx: Arc::clone(&self.ws_tx),
        }
    }
}

impl SyncEngine {
    // ----- Transport registration -----

    /// Install the WebSocket delivery hook. Called once at webserver startup.
    pub fn set_ws_sender(&self, tx: mpsc::UnboundedSender<WsBroadcast>) {
        let _ = self.ws_tx.set(tx);
    }

    pub async fn register_user_addr(&self, user_id: i64, addr: SocketAddr) {
        self.state.write().await.user_addrs.insert(user_id, addr);
    }

    pub async fn unregister_user_addr(&self, user_id: i64) {
        self.state.write().await.user_addrs.remove(&user_id);
    }

    pub async fn allocate_conn_id(&self) -> u64 {
        let mut state = self.state.write().await;
        let id = state.next_conn_id;
        state.next_conn_id += 1;
        id
    }

    pub async fn register_syncplay_conn(&self, conn_id: u64, tx: mpsc::UnboundedSender<String>) {
        self.state.write().await.syncplay_conns.insert(conn_id, tx);
    }

    pub async fn unregister_syncplay_conn(&self, conn_id: u64) {
        self.state.write().await.syncplay_conns.remove(&conn_id);
    }

    /// Deliver notifications on both transports. WS broadcasts go through the
    /// hook installed by the web server; Syncplay messages go to the
    /// registered TCP connection channels.
    pub async fn dispatch(&self, notifs: Vec<SyncNotification>) {
        if notifs.is_empty() {
            return;
        }

        let state = self.state.read().await;
        for notif in notifs {
            match notif {
                SyncNotification::WebSocket(b) => {
                    if let Some(tx) = self.ws_tx.get() {
                        let _ = tx.send(b);
                    }
                }
                SyncNotification::Syncplay(sp) => {
                    for conn_id in &sp.conn_ids {
                        if let Some(tx) = state.syncplay_conns.get(conn_id) {
                            let _ = tx.send(sp.message.clone());
                        }
                    }
                }
            }
        }
    }

    // ----- Room CRUD -----

    pub async fn create_room(
        &self,
        media_file_id: Option<i64>,
        media_id: Option<i64>,
        media_name: String,
        creator: ParticipantId,
        display_name: String,
        kind: ParticipantKind,
        control_mode: Option<ControlMode>,
        password: Option<String>,
    ) -> RoomInfo {
        let mut state = self.state.write().await;
        create_room_locked(
            &mut state,
            media_file_id,
            media_id,
            media_name,
            creator,
            display_name,
            kind,
            control_mode,
            password,
            None,
        )
    }

    pub async fn join_room(
        &self,
        participant: SyncParticipant,
        code: &str,
        password: Option<&str>,
    ) -> Result<(RoomInfo, Vec<SyncNotification>), &'static str> {
        let mut state = self.state.write().await;
        join_room_locked(&mut state, participant, code, password)
    }

    /// Join or create a room by name (used by Syncplay clients). Atomic:
    /// concurrent joins to the same new name cannot create duplicate rooms.
    pub async fn join_room_by_name(
        &self,
        participant: SyncParticipant,
        room_name: &str,
        password: Option<&str>,
    ) -> Result<(String, RoomInfo, Vec<SyncNotification>), &'static str> {
        let mut state = self.state.write().await;

        let existing_code = if state.rooms.contains_key(room_name) {
            Some(room_name.to_string())
        } else {
            state.room_name_aliases.get(room_name).cloned()
        };

        if let Some(code) = existing_code {
            let (info, notifs) = join_room_locked(&mut state, participant, &code, password)?;
            return Ok((code, info, notifs));
        }

        // Room doesn't exist — create it with Egalitarian mode.
        let pid = participant.id;
        let info = create_room_locked(
            &mut state,
            None,
            None,
            room_name.to_string(),
            pid,
            participant.display_name.clone(),
            participant.kind.clone(),
            Some(ControlMode::Egalitarian),
            password.map(|s| s.to_string()),
            Some(room_name.to_string()),
        );
        let code = info.code.clone();

        Ok((code, info, vec![]))
    }

    pub async fn leave_room(
        &self,
        participant_id: ParticipantId,
        code: &str,
    ) -> Result<Option<Vec<SyncNotification>>, &'static str> {
        let mut state = self.state.write().await;
        if !state.rooms.contains_key(code) {
            return Err("Room not found");
        }
        Ok(remove_participant_locked(&mut state, participant_id, code))
    }

    pub async fn get_room_info(&self, code: &str) -> Option<RoomInfo> {
        let state = self.state.read().await;
        state.rooms.get(code).map(|r| r.to_info())
    }

    /// Full Syncplay user list for a room (reply to a `List` request).
    pub async fn syncplay_list(&self, code: &str) -> Option<String> {
        let state = self.state.read().await;
        state.rooms.get(code).map(make_syncplay_list)
    }

    pub async fn list_rooms(&self) -> Vec<RoomInfo> {
        let state = self.state.read().await;
        state.rooms.values().map(|r| r.to_info()).collect()
    }

    pub async fn transfer_host(
        &self,
        code: &str,
        from_id: ParticipantId,
        to_id: ParticipantId,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        if room.host_id() != Some(from_id) {
            return Err("Only the host can transfer host");
        }
        if !room.participants.iter().any(|p| p.id == to_id) {
            return Err("Target user not in room");
        }

        for p in &mut room.participants {
            p.is_host = p.id == to_id;
        }

        let ws_msg = make_room_update_message(room);
        Ok(ws_broadcast_to_room(user_addrs, room, &ws_msg, None))
    }

    // ----- Playback control -----

    pub async fn set_playback(
        &self,
        participant_id: ParticipantId,
        code: &str,
        action: &str,
        position: f64,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        if !room.can_control(participant_id) {
            return Err("Not allowed to control playback");
        }

        match action {
            "play" => {
                room.playback_state = PlaybackState::Playing;
                room.playback_position = position;
                room.last_position_update = Instant::now();
            }
            "pause" => {
                room.playback_state = PlaybackState::Paused;
                room.playback_position = position;
                room.last_position_update = Instant::now();
            }
            "seek" => {
                room.playback_position = position;
                room.last_position_update = Instant::now();
            }
            _ => return Err("Invalid action"),
        }

        // Exclude the sender — they already applied the action locally.
        // Echoing it back causes a feedback loop: the sender would seek
        // to their own position, triggering buffering events that cascade.
        let ws_msg = make_sync_message(code, action, position);
        let mut notifs = ws_broadcast_to_room(user_addrs, room, &ws_msg, Some(participant_id));

        // Notify Syncplay TCP clients (except the sender, if the sender is
        // one). A "seek" must carry doSeek so clients hard-jump.
        let set_by = room.participant_name(participant_id);
        let sp_state =
            make_syncplay_state_message(room, action == "seek", set_by.as_deref());
        notifs.extend(sp_broadcast_to_room(room, sp_state, Some(participant_id)));

        Ok(notifs)
    }

    /// Handle a Syncplay client's State report.
    ///
    /// `playstate` is `Some((position, paused, do_seek))` when the client
    /// included playback state, `None` for ping-only reports. Returns the
    /// authoritative room snapshot (for the per-connection reply) plus any
    /// notifications for OTHER participants (only produced when the report
    /// actually changed room state).
    pub async fn report_state(
        &self,
        participant_id: ParticipantId,
        code: &str,
        playstate: Option<(f64, bool, bool)>,
        latency_ms: u32,
    ) -> Result<(RoomPlaybackSnapshot, Vec<SyncNotification>), &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        // Update participant latency
        if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
            p.latency_ms = latency_ms;
        }

        let mut notifs = Vec::new();

        if let Some((position, paused, do_seek)) = playstate {
            if room.can_control(participant_id) {
                let current_paused = room.playback_state == PlaybackState::Paused;
                let pause_toggled = paused != current_paused;

                if pause_toggled {
                    room.playback_state = if paused {
                        PlaybackState::Paused
                    } else {
                        PlaybackState::Playing
                    };
                    room.playback_position = position;
                    room.last_position_update = Instant::now();
                } else if do_seek {
                    room.playback_position = position;
                    room.last_position_update = Instant::now();
                } else if !paused
                    && (position - room.current_position()).abs() < DRIFT_ADOPT_THRESHOLD_SECS
                {
                    // Normal playback drift from a controlling client —
                    // silently adopt so the authoritative position tracks
                    // real playback (buffer stalls, clock skew, ...).
                    room.playback_position = position;
                    room.last_position_update = Instant::now();
                }

                if pause_toggled || do_seek {
                    let action = if pause_toggled {
                        if paused { "pause" } else { "play" }
                    } else {
                        "seek"
                    };

                    let set_by = room.participant_name(participant_id);
                    let ws_msg = make_sync_message(code, action, position);
                    notifs.extend(ws_broadcast_to_room(
                        user_addrs,
                        room,
                        &ws_msg,
                        Some(participant_id),
                    ));

                    let sp_state =
                        make_syncplay_state_message(room, do_seek, set_by.as_deref());
                    notifs.extend(sp_broadcast_to_room(room, sp_state, Some(participant_id)));
                }
            }
        }

        let snapshot = RoomPlaybackSnapshot {
            position: room.current_position(),
            paused: room.playback_state == PlaybackState::Paused,
        };

        Ok((snapshot, notifs))
    }

    // ----- File info -----

    pub async fn set_file(
        &self,
        participant_id: ParticipantId,
        code: &str,
        file: ParticipantFile,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
            p.file = Some(file);
        } else {
            return Err("User not in room");
        }

        let mut notifs = Vec::new();

        // Tell other Syncplay clients about the file change.
        if let Some(p) = room.participants.iter().find(|p| p.id == participant_id) {
            let sp_msg = make_syncplay_user_event(room, p, None);
            notifs.extend(sp_broadcast_to_room(room, sp_msg, Some(participant_id)));
        }

        // Also notify WS clients
        let ws_msg = make_room_update_message(room);
        notifs.extend(ws_broadcast_to_room(user_addrs, room, &ws_msg, None));

        Ok(notifs)
    }

    // ----- Buffering -----

    pub async fn set_buffering(
        &self,
        participant_id: ParticipantId,
        code: &str,
        username: &str,
        is_buffering: bool,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        let user_id = if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id)
        {
            p.is_buffering = is_buffering;
            p.user_id()
        } else {
            return Err("User not in room");
        };

        // Only broadcast the buffering status notification.
        // We do NOT auto-pause/resume the room based on buffering state because
        // it creates a feedback loop: pause → seek → buffer → buffering=true →
        // pause → ... The host can manually pause/resume if needed.
        let ws_msg = make_buffering_message(code, user_id, username, is_buffering);
        Ok(ws_broadcast_to_room(user_addrs, room, &ws_msg, None))
    }

    // ----- Chat -----

    pub async fn add_chat(
        &self,
        participant_id: ParticipantId,
        code: &str,
        username: &str,
        text: String,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        let user_id = match room.participants.iter().find(|p| p.id == participant_id) {
            Some(p) => p.user_id(),
            None => return Err("User not in room"),
        };

        let timestamp_ms = now_ms();
        room.chat_messages.push(ChatMessage {
            user_id,
            username: username.to_string(),
            text: text.clone(),
            timestamp_ms,
        });

        if room.chat_messages.len() > MAX_CHAT_MESSAGES {
            room.chat_messages.remove(0);
        }

        let ws_msg = make_chat_message_event(code, user_id, username, &text, timestamp_ms);
        let mut notifs = ws_broadcast_to_room(user_addrs, room, &ws_msg, None);

        let sp_msg = make_syncplay_chat_message(username, &text);
        notifs.extend(sp_broadcast_to_room(room, sp_msg, Some(participant_id)));

        Ok(notifs)
    }

    // ----- Ready status -----

    pub async fn set_ready(
        &self,
        participant_id: ParticipantId,
        code: &str,
        is_ready: bool,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        let (user_id, username) =
            if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
                p.is_ready = is_ready;
                (p.user_id(), p.display_name.clone())
            } else {
                return Err("User not in room");
            };

        let ws_msg = make_ready_message(code, user_id, &username, is_ready);
        let mut notifs = ws_broadcast_to_room(user_addrs, room, &ws_msg, None);

        let sp_msg = make_syncplay_ready(&username, is_ready);
        notifs.extend(sp_broadcast_to_room(room, sp_msg, Some(participant_id)));

        Ok(notifs)
    }

    // ----- Control mode -----

    pub async fn set_control_mode(
        &self,
        participant_id: ParticipantId,
        code: &str,
        mode: ControlMode,
    ) -> Result<Vec<SyncNotification>, &'static str> {
        let mut state = self.state.write().await;
        let EngineState { rooms, user_addrs, .. } = &mut *state;
        let room = rooms.get_mut(code).ok_or("Room not found")?;

        if room.host_id() != Some(participant_id) {
            return Err("Only the host can change control mode");
        }

        room.control_mode = mode;

        let ws_msg = make_mode_changed_message(code, room.control_mode.as_str());
        let mut notifs = ws_broadcast_to_room(user_addrs, room, &ws_msg, None);

        let update_msg = make_room_update_message(room);
        notifs.extend(ws_broadcast_to_room(user_addrs, room, &update_msg, None));

        Ok(notifs)
    }

    // ----- Disconnect handling -----

    /// Remove a participant after their transport dropped. Used by the
    /// Syncplay TCP server, where conn ids are unique per connection.
    pub async fn handle_disconnect(
        &self,
        participant_id: ParticipantId,
    ) -> Option<Vec<SyncNotification>> {
        let mut state = self.state.write().await;

        let code = state
            .rooms
            .values()
            .find(|r| r.participants.iter().any(|p| p.id == participant_id))
            .map(|r| r.code.clone())?;

        remove_participant_locked(&mut state, participant_id, &code)
    }

    /// WebSocket-scoped disconnect. Only acts when `addr` is still the
    /// registered socket for this user — a stale socket closing after the
    /// user reconnected must NOT kick the live connection out of the room.
    pub async fn handle_ws_disconnect(
        &self,
        user_id: i64,
        addr: SocketAddr,
    ) -> Option<Vec<SyncNotification>> {
        let mut state = self.state.write().await;

        match state.user_addrs.get(&user_id) {
            Some(current) if *current == addr => {
                state.user_addrs.remove(&user_id);
            }
            // A newer socket owns this user now (reconnect), or the user was
            // never registered — nothing to clean up.
            _ => return None,
        }

        let pid = ParticipantId::DimUser(user_id);
        let code = state
            .rooms
            .values()
            .find(|r| r.participants.iter().any(|p| p.id == pid))
            .map(|r| r.code.clone())?;

        remove_participant_locked(&mut state, pid, &code)
    }
}

// ---------------------------------------------------------------------------
// Lock-held helpers (shared by multiple engine methods)
// ---------------------------------------------------------------------------

fn create_room_locked(
    state: &mut EngineState,
    media_file_id: Option<i64>,
    media_id: Option<i64>,
    media_name: String,
    creator: ParticipantId,
    display_name: String,
    kind: ParticipantKind,
    control_mode: Option<ControlMode>,
    password: Option<String>,
    room_name: Option<String>,
) -> RoomInfo {
    let code = loop {
        let code = generate_room_code();
        if !state.rooms.contains_key(&code) {
            break code;
        }
    };

    let mode = control_mode.unwrap_or(ControlMode::HostOnly);
    let room_name = room_name.unwrap_or_else(|| code.clone());

    let participant = SyncParticipant {
        id: creator,
        display_name,
        kind,
        is_host: true,
        is_ready: false,
        is_buffering: false,
        latency_ms: 0,
        file: None,
    };

    let room = SyncRoom {
        code: code.clone(),
        name: room_name.clone(),
        password,
        control_mode: mode,
        media_file_id,
        media_id,
        media_name,
        playback_state: PlaybackState::Paused,
        playback_position: 0.0,
        last_position_update: Instant::now(),
        participants: vec![participant],
        chat_messages: Vec::new(),
    };

    let info = room.to_info();
    state.room_name_aliases.insert(room_name, code.clone());
    state.rooms.insert(code, room);
    info
}

fn join_room_locked(
    state: &mut EngineState,
    mut participant: SyncParticipant,
    code: &str,
    password: Option<&str>,
) -> Result<(RoomInfo, Vec<SyncNotification>), &'static str> {
    let EngineState { rooms, user_addrs, .. } = &mut *state;
    let room = rooms.get_mut(code).ok_or("Room not found")?;

    // Check password
    if let Some(ref room_pw) = room.password {
        if !room_pw.is_empty() {
            match password {
                Some(pw) if password_matches(pw, room_pw) => {}
                _ => return Err("Invalid password"),
            }
        }
    }

    let pid = participant.id;
    let mut notifs = Vec::new();

    if !room.participants.iter().any(|p| p.id == pid) {
        // Uniquify the display name — the Syncplay protocol keys user state
        // by name, so duplicates would collide.
        let base_name = participant.display_name.clone();
        let mut n = 2;
        while room
            .participants
            .iter()
            .any(|p| p.display_name == participant.display_name)
        {
            participant.display_name = format!("{} ({})", base_name, n);
            n += 1;
        }

        room.participants.push(participant);

        // Tell Syncplay clients someone joined (official Set.user event).
        if let Some(p) = room.participants.last() {
            let sp_msg = make_syncplay_user_event(room, p, Some("joined"));
            notifs.extend(sp_broadcast_to_room(room, sp_msg, Some(pid)));
        }
    }

    let info = room.to_info();
    let ws_msg = make_room_update_message(room);
    notifs.extend(ws_broadcast_to_room(user_addrs, room, &ws_msg, None));

    Ok((info, notifs))
}

/// Remove a participant from a room. If they were the host and others remain,
/// host is migrated to the next participant instead of destroying the room —
/// a transient host disconnect must never kick everyone. The room is only
/// destroyed when it becomes empty.
fn remove_participant_locked(
    state: &mut EngineState,
    participant_id: ParticipantId,
    code: &str,
) -> Option<Vec<SyncNotification>> {
    let EngineState {
        rooms,
        user_addrs,
        room_name_aliases,
        syncplay_conns,
        ..
    } = &mut *state;

    let room = rooms.get_mut(code)?;

    let leaving = room
        .participants
        .iter()
        .find(|p| p.id == participant_id)
        .cloned()?;
    room.participants.retain(|p| p.id != participant_id);

    if room.participants.is_empty() {
        // Nobody left to notify; emit the destroyed event anyway for
        // completeness (no targets → no messages) and clean up.
        let ws_msg = make_room_destroyed_message(code, "empty");
        let notifs = ws_broadcast_to_room(user_addrs, room, &ws_msg, None);

        for conn_id in room.syncplay_conn_ids() {
            syncplay_conns.remove(&conn_id);
        }
        room_name_aliases.retain(|_, v| v != code);
        rooms.remove(code);

        return if notifs.is_empty() { None } else { Some(notifs) };
    }

    // Migrate host if needed.
    if leaving.is_host {
        if let Some(next) = room.participants.first_mut() {
            next.is_host = true;
        }
    }

    let mut notifs = Vec::new();

    // Tell Syncplay clients someone left (official Set.user event).
    let sp_msg = make_syncplay_user_event(room, &leaving, Some("left"));
    notifs.extend(sp_broadcast_to_room(room, sp_msg, None));

    let ws_msg = make_room_update_message(room);
    notifs.extend(ws_broadcast_to_room(user_addrs, room, &ws_msg, None));

    Some(notifs)
}
