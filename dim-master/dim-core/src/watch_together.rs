//! Thin compatibility wrapper around SyncEngine.
//!
//! Preserves the original `WatchTogetherManager` API so existing callers
//! (REST routes, WebSocket handler) continue to work without changes.
//!
//! All methods dispatch the resulting notifications on BOTH transports
//! (WebSocket via the engine's installed sender hook, Syncplay via the
//! engine's connection registry) — callers no longer deliver anything
//! themselves.

use std::net::SocketAddr;

use crate::sync_engine::{
    ControlMode, ParticipantId, ParticipantKind, SyncEngine, SyncParticipant,
};

/// Re-export RoomInfo from sync_engine.
pub use crate::sync_engine::RoomInfo;

#[derive(Debug)]
pub struct WatchTogetherManager {
    engine: SyncEngine,
}

impl WatchTogetherManager {
    pub fn new(engine: SyncEngine) -> Self {
        Self { engine }
    }

    pub fn engine(&self) -> &SyncEngine {
        &self.engine
    }
}

impl Default for WatchTogetherManager {
    fn default() -> Self {
        Self {
            engine: SyncEngine::default(),
        }
    }
}

impl Clone for WatchTogetherManager {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
        }
    }
}

impl WatchTogetherManager {
    pub async fn register_user_addr(&self, user_id: i64, addr: SocketAddr) {
        self.engine.register_user_addr(user_id, addr).await;
    }

    pub async fn unregister_user_addr(&self, user_id: i64) {
        self.engine.unregister_user_addr(user_id).await;
    }

    pub async fn create_room(
        &self,
        media_file_id: i64,
        media_id: i64,
        media_name: String,
        user_id: i64,
        username: String,
        picture: Option<i64>,
    ) -> RoomInfo {
        self.engine
            .create_room(
                Some(media_file_id),
                Some(media_id),
                media_name,
                ParticipantId::DimUser(user_id),
                username,
                ParticipantKind::DimUser { user_id, picture },
                None,
                None,
            )
            .await
    }

    pub async fn create_room_with_options(
        &self,
        media_file_id: i64,
        media_id: i64,
        media_name: String,
        user_id: i64,
        username: String,
        picture: Option<i64>,
        control_mode: Option<ControlMode>,
        password: Option<String>,
    ) -> RoomInfo {
        self.engine
            .create_room(
                Some(media_file_id),
                Some(media_id),
                media_name,
                ParticipantId::DimUser(user_id),
                username,
                ParticipantKind::DimUser { user_id, picture },
                control_mode,
                password,
            )
            .await
    }

    pub async fn join_room(
        &self,
        code: &str,
        user_id: i64,
        username: String,
        picture: Option<i64>,
    ) -> Result<RoomInfo, &'static str> {
        self.join_room_with_password(code, user_id, username, picture, None)
            .await
    }

    pub async fn join_room_with_password(
        &self,
        code: &str,
        user_id: i64,
        username: String,
        picture: Option<i64>,
        password: Option<&str>,
    ) -> Result<RoomInfo, &'static str> {
        let participant = SyncParticipant {
            id: ParticipantId::DimUser(user_id),
            display_name: username,
            kind: ParticipantKind::DimUser { user_id, picture },
            is_host: false,
            is_ready: false,
            is_buffering: false,
            latency_ms: 0,
            file: None,
        };

        let (info, notifs) = self.engine.join_room(participant, code, password).await?;
        self.engine.dispatch(notifs).await;
        Ok(info)
    }

    pub async fn leave_room(&self, code: &str, user_id: i64) -> Result<(), &'static str> {
        let result = self
            .engine
            .leave_room(ParticipantId::DimUser(user_id), code)
            .await?;
        if let Some(notifs) = result {
            self.engine.dispatch(notifs).await;
        }
        Ok(())
    }

    pub async fn get_room_info(&self, code: &str) -> Option<RoomInfo> {
        self.engine.get_room_info(code).await
    }

    pub async fn list_rooms(&self) -> Vec<RoomInfo> {
        self.engine.list_rooms().await
    }

    pub async fn transfer_host(
        &self,
        code: &str,
        from_user_id: i64,
        to_user_id: i64,
    ) -> Result<(), &'static str> {
        let notifs = self
            .engine
            .transfer_host(
                code,
                ParticipantId::DimUser(from_user_id),
                ParticipantId::DimUser(to_user_id),
            )
            .await?;
        self.engine.dispatch(notifs).await;
        Ok(())
    }

    pub async fn set_playback(
        &self,
        code: &str,
        user_id: i64,
        action: &str,
        position: f64,
    ) -> Result<(), &'static str> {
        let notifs = self
            .engine
            .set_playback(ParticipantId::DimUser(user_id), code, action, position)
            .await?;
        self.engine.dispatch(notifs).await;
        Ok(())
    }

    pub async fn set_buffering(
        &self,
        code: &str,
        user_id: i64,
        username: &str,
        is_buffering: bool,
    ) -> Result<(), &'static str> {
        let notifs = self
            .engine
            .set_buffering(
                ParticipantId::DimUser(user_id),
                code,
                username,
                is_buffering,
            )
            .await?;
        self.engine.dispatch(notifs).await;
        Ok(())
    }

    pub async fn add_chat_message(
        &self,
        code: &str,
        user_id: i64,
        username: &str,
        text: String,
    ) -> Result<(), &'static str> {
        let notifs = self
            .engine
            .add_chat(ParticipantId::DimUser(user_id), code, username, text)
            .await?;
        self.engine.dispatch(notifs).await;
        Ok(())
    }

    pub async fn set_ready(
        &self,
        code: &str,
        user_id: i64,
        is_ready: bool,
    ) -> Result<(), &'static str> {
        let notifs = self
            .engine
            .set_ready(ParticipantId::DimUser(user_id), code, is_ready)
            .await?;
        self.engine.dispatch(notifs).await;
        Ok(())
    }

    pub async fn set_control_mode(
        &self,
        code: &str,
        user_id: i64,
        mode: ControlMode,
    ) -> Result<(), &'static str> {
        let notifs = self
            .engine
            .set_control_mode(ParticipantId::DimUser(user_id), code, mode)
            .await?;
        self.engine.dispatch(notifs).await;
        Ok(())
    }

    /// Socket-scoped disconnect cleanup. Only removes the participant when
    /// `addr` is still the user's registered socket (a stale socket closing
    /// after a reconnect is a no-op).
    pub async fn handle_ws_disconnect(&self, user_id: i64, addr: SocketAddr) {
        if let Some(notifs) = self.engine.handle_ws_disconnect(user_id, addr).await {
            self.engine.dispatch(notifs).await;
        }
    }
}
