use super::*;

fn participant(user_id: i64) -> SyncParticipant {
    SyncParticipant {
        id: ParticipantId::DimUser(user_id),
        display_name: format!("user{user_id}"),
        kind: ParticipantKind::DimUser {
            user_id,
            picture: None,
        },
        is_host: false,
        is_ready: false,
        is_buffering: false,
        latency_ms: 0,
        file: None,
    }
}

async fn room(engine: &SyncEngine) -> RoomInfo {
    engine
        .create_room(
            Some(1),
            Some(1),
            "Test".into(),
            ParticipantId::DimUser(1),
            "user1".into(),
            participant(1).kind,
            None,
            Some("secret".into()),
        )
        .await
}

#[tokio::test]
async fn changing_episode_keeps_the_room_and_notifies_every_viewer() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    let host = "127.0.0.1:1000".parse().unwrap();
    let guest = "127.0.0.1:1001".parse().unwrap();
    engine.register_user_addr(1, host).await;
    engine.register_user_addr(2, guest).await;
    engine
        .add_chat(
            ParticipantId::DimUser(2),
            &r.code,
            "user2",
            "Keep this chat".into(),
        )
        .await
        .unwrap();
    engine
        .set_playback(ParticipantId::DimUser(1), &r.code, "play", 1800.0)
        .await
        .unwrap();

    let (updated, notifications) = engine
        .change_media(
            ParticipantId::DimUser(1),
            &r.code,
            1,
            2,
            22,
            "Episode 2".into(),
        )
        .await
        .unwrap();
    assert_eq!(updated.code, r.code);
    assert_eq!(updated.media_file_id, Some(2));
    assert_eq!(updated.media_id, Some(22));
    assert_eq!(updated.host_user_id, 1);
    assert_eq!(updated.participants.len(), 2);
    // Nobody has the new episode loaded yet; the host's report starts it.
    assert_eq!(updated.playback_state, "paused");
    assert_eq!(updated.playback_position, 0.0);
    let state = engine.state.read().await;
    let room = state.rooms.get(&r.code).unwrap();
    assert_eq!(room.password.as_deref(), Some("secret"));
    assert_eq!(room.chat_messages.len(), 1);
    let mut targets = Vec::new();
    for notification in notifications {
        if let SyncNotification::WebSocket(broadcast) = notification {
            targets.extend(broadcast.targets);
            let msg: serde_json::Value = serde_json::from_str(&broadcast.message).unwrap();
            assert_eq!(msg["type"], "EventWatchTogetherRoomUpdate");
            assert_eq!(msg["media_file_id"], 2);
        }
    }
    assert!(targets.contains(&host));
    assert!(targets.contains(&guest));
}

#[tokio::test]
async fn only_host_can_advance_and_stale_episode_requests_do_not_reset_playback() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    // Shared playback controls must not make competing autoplay leaders.
    engine
        .set_control_mode(ParticipantId::DimUser(1), &r.code, ControlMode::Egalitarian)
        .await
        .unwrap();
    for user in [2, 3] {
        assert!(engine
            .change_media(
                ParticipantId::DimUser(user),
                &r.code,
                1,
                2,
                2,
                "Next".into()
            )
            .await
            .is_err());
    }
    engine
        .change_media(ParticipantId::DimUser(1), &r.code, 1, 2, 2, "Next".into())
        .await
        .unwrap();
    engine
        .set_playback(ParticipantId::DimUser(1), &r.code, "pause", 25.0)
        .await
        .unwrap();
    assert!(engine
        .report_host_state(ParticipantId::DimUser(1), &r.code, 1800.0, true, Some(1))
        .await
        .is_err());
    assert!(engine
        .change_media(ParticipantId::DimUser(1), &r.code, 1, 2, 2, "Next".into())
        .await
        .is_err());
    let info = engine.get_room_info(&r.code).await.unwrap();
    assert_eq!(info.media_file_id, Some(2));
    assert_eq!(info.playback_position, 25.0);
    assert_eq!(info.playback_state, "paused");
}

#[tokio::test]
async fn disconnect_removes_every_membership_and_migrates_remaining_hosts() {
    let engine = SyncEngine::default();
    let a = room(&engine).await;
    let b = room(&engine).await;
    engine
        .join_room(participant(2), &a.code, Some("secret"))
        .await
        .unwrap();
    let addr = "127.0.0.1:1000".parse().unwrap();
    engine.register_user_addr(1, addr).await;
    engine.handle_ws_disconnect(1, addr).await;
    assert!(engine.get_room_info(&b.code).await.is_none());
    let remaining = engine.get_room_info(&a.code).await.unwrap();
    assert_eq!(remaining.participants.len(), 1);
    assert_eq!(remaining.host_user_id, 2);
}

#[tokio::test]
async fn reconnect_before_cleanup_preserves_host_and_after_cleanup_can_rejoin() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    let old = "127.0.0.1:1000".parse().unwrap();
    let new = "127.0.0.1:1001".parse().unwrap();
    engine.register_user_addr(1, old).await;
    engine.register_user_addr(1, new).await;
    engine.handle_ws_disconnect(1, old).await;
    assert_eq!(engine.get_room_info(&r.code).await.unwrap().host_user_id, 1);
    engine.handle_ws_disconnect(1, new).await;
    engine.register_user_addr(1, old).await;
    let (info, _) = engine
        .join_room(participant(1), &r.code, Some("secret"))
        .await
        .unwrap();
    assert_eq!(info.participants.len(), 2);
    assert_eq!(info.host_user_id, 2);
    assert!(engine
        .add_chat(ParticipantId::DimUser(1), &r.code, "user1", "back".into())
        .await
        .is_ok());
}

#[tokio::test]
async fn host_reports_pause_stalls_and_resume_from_actual_position_without_echo() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    let host = "127.0.0.1:1000".parse().unwrap();
    let guest = "127.0.0.1:1001".parse().unwrap();
    engine.register_user_addr(1, host).await;
    engine.register_user_addr(2, guest).await;
    engine
        .set_playback(ParticipantId::DimUser(1), &r.code, "play", 100.0)
        .await
        .unwrap();
    let notifications = engine
        .report_host_state(ParticipantId::DimUser(1), &r.code, 101.0, true, Some(1))
        .await
        .unwrap();
    assert_eq!(notifications.len(), 1);
    if let SyncNotification::WebSocket(msg) = &notifications[0] {
        assert_eq!(msg.targets, vec![guest]);
        assert!(msg.message.contains("\"pause\""));
    } else {
        panic!("expected WebSocket notification");
    }
    {
        let mut state = engine.state.write().await;
        state.rooms.get_mut(&r.code).unwrap().last_position_update =
            Instant::now() - std::time::Duration::from_secs(20);
    }
    let stalled = engine.get_room_info(&r.code).await.unwrap();
    assert_eq!(stalled.playback_position, 101.0);
    assert_eq!(stalled.playback_state, "paused");
    engine
        .report_host_state(ParticipantId::DimUser(1), &r.code, 101.0, false, Some(1))
        .await
        .unwrap();
    assert_eq!(
        engine.get_room_info(&r.code).await.unwrap().playback_state,
        "playing"
    );
    // Large actual-position corrections are adopted without triggering seeks.
    assert!(engine
        .report_host_state(ParticipantId::DimUser(1), &r.code, 50.0, false, Some(1))
        .await
        .unwrap()
        .is_empty());
    assert!(
        engine
            .get_room_info(&r.code)
            .await
            .unwrap()
            .playback_position
            < 51.0
    );
}

#[tokio::test]
async fn host_reports_reject_guests_even_in_egalitarian_rooms_and_invalid_positions() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    engine
        .set_control_mode(ParticipantId::DimUser(1), &r.code, ControlMode::Egalitarian)
        .await
        .unwrap();
    assert!(engine
        .report_host_state(ParticipantId::DimUser(2), &r.code, 5.0, false, Some(1))
        .await
        .is_err());
    for position in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(engine
            .report_host_state(ParticipantId::DimUser(1), &r.code, position, false, Some(1))
            .await
            .is_err());
    }
    assert_eq!(
        engine
            .get_room_info(&r.code)
            .await
            .unwrap()
            .playback_position,
        0.0
    );
    assert!(matches!(
        engine
            .join_room(participant(3), &r.code, Some("wrong"))
            .await,
        Err("Invalid password")
    ));
}

fn syncplay_participant(conn_id: u64, name: &str) -> SyncParticipant {
    SyncParticipant {
        id: ParticipantId::Syncplay(conn_id),
        display_name: name.into(),
        kind: ParticipantKind::SyncplayClient { conn_id },
        is_host: false,
        is_ready: false,
        is_buffering: false,
        latency_ms: 0,
        file: None,
    }
}

#[tokio::test]
async fn closing_a_second_tab_keeps_the_watching_tab_in_the_room() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    let watching = "127.0.0.1:1000".parse().unwrap();
    let browsing = "127.0.0.1:1001".parse().unwrap();
    engine.register_user_addr(2, watching).await;
    engine.register_user_addr(2, browsing).await;

    // Both tabs receive room traffic.
    let notifs = engine
        .add_chat(ParticipantId::DimUser(1), &r.code, "user1", "hi".into())
        .await
        .unwrap();
    let SyncNotification::WebSocket(b) = &notifs[0] else {
        panic!("expected WebSocket notification")
    };
    assert!(b.targets.contains(&watching) && b.targets.contains(&browsing));

    assert!(engine.handle_ws_disconnect(2, browsing).await.is_none());
    assert_eq!(engine.get_room_info(&r.code).await.unwrap().participants.len(), 2);
    engine.handle_ws_disconnect(2, watching).await;
    assert_eq!(engine.get_room_info(&r.code).await.unwrap().participants.len(), 1);
}

#[tokio::test]
async fn stale_host_report_cannot_undo_a_guest_pause() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    engine
        .set_control_mode(ParticipantId::DimUser(1), &r.code, ControlMode::Egalitarian)
        .await
        .unwrap();
    engine
        .set_playback(ParticipantId::DimUser(1), &r.code, "play", 10.0)
        .await
        .unwrap();
    engine
        .set_playback(ParticipantId::DimUser(2), &r.code, "pause", 12.0)
        .await
        .unwrap();
    // The host's report was sent before it saw the pause.
    assert!(engine
        .report_host_state(ParticipantId::DimUser(1), &r.code, 11.5, false, Some(1))
        .await
        .unwrap()
        .is_empty());
    let info = engine.get_room_info(&r.code).await.unwrap();
    assert_eq!(info.playback_state, "paused");
    assert_eq!(info.playback_position, 12.0);

    // Once the hold-off expires the host's reports count again.
    {
        let mut state = engine.state.write().await;
        state.rooms.get_mut(&r.code).unwrap().host_report_holdoff_until = None;
    }
    engine
        .report_host_state(ParticipantId::DimUser(1), &r.code, 12.0, false, Some(1))
        .await
        .unwrap();
    assert_eq!(engine.get_room_info(&r.code).await.unwrap().playback_state, "playing");
}

#[tokio::test]
async fn host_migration_prefers_dim_users_over_syncplay_clients() {
    let engine = SyncEngine::default();
    let r = room(&engine).await;
    engine
        .join_room(syncplay_participant(7, "vlc"), &r.code, Some("secret"))
        .await
        .unwrap();
    engine
        .join_room(participant(2), &r.code, Some("secret"))
        .await
        .unwrap();
    engine
        .leave_room(ParticipantId::DimUser(1), &r.code)
        .await
        .unwrap();
    assert_eq!(engine.get_room_info(&r.code).await.unwrap().host_user_id, 2);
}

#[tokio::test]
async fn syncplay_server_password_does_not_become_a_room_password() {
    let engine = SyncEngine::default();
    let digest = format!("{:x}", md5::compute(b"serverpw"));
    let (code, _, _) = engine
        .join_room_by_name(syncplay_participant(1, "vlc"), "movie night", Some(&digest))
        .await
        .unwrap();
    assert!(engine.join_room(participant(2), &code, None).await.is_ok());
}
