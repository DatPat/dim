use crate::AppState;
use axum::extract::{Json, Path, State};
use axum::response::IntoResponse;
use axum::Extension;

use dim_database::user::User;
use http::StatusCode;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateRoomBody {
    pub media_file_id: i64,
    pub media_id: i64,
    pub media_name: String,
    #[serde(default)]
    pub control_mode: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Deserialize)]
pub struct TransferHostBody {
    pub to_user_id: i64,
}

#[derive(Deserialize)]
pub struct ReadyBody {
    pub is_ready: bool,
}

#[derive(Deserialize)]
pub struct ChangeMediaBody {
    pub expected_media_file_id: i64,
    pub media_file_id: i64,
}

pub async fn change_media(
    State(AppState {
        watch_together,
        conn,
        ..
    }): State<AppState>,
    Extension(user): Extension<User>,
    Path(code): Path<String>,
    Json(body): Json<ChangeMediaBody>,
) -> Result<Json<dim_core::sync_engine::RoomInfo>, (StatusCode, String)> {
    let mut tx = conn.read().begin().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let file = dim_database::mediafile::MediaFile::get_one(&mut tx, body.media_file_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Media file not found".to_string()))?;
    let media_id = file
        .media_id
        .ok_or((StatusCode::BAD_REQUEST, "Media file has no media".to_string()))?;
    let media = dim_database::media::Media::get(&mut tx, media_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Media not found".to_string()))?;
    let (info, notifications) = watch_together
        .engine()
        .change_media(
            dim_core::sync_engine::ParticipantId::DimUser(user.id.0),
            &code,
            body.expected_media_file_id,
            file.id,
            media_id,
            media.name,
        )
        .await
        .map_err(|e| (
            match e {
                "Room not found" => StatusCode::NOT_FOUND,
                "Room media has changed" => StatusCode::CONFLICT,
                _ => StatusCode::FORBIDDEN,
            },
            e.to_string(),
        ))?;
    watch_together.engine().dispatch(notifications).await;
    Ok(Json(info))
}

#[derive(Deserialize)]
pub struct JoinRoomBody {
    #[serde(default)]
    pub password: Option<String>,
}

pub async fn create_room(
    State(AppState { watch_together, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateRoomBody>,
) -> impl IntoResponse {
    let control_mode = body.control_mode.as_deref().and_then(|m| match m {
        "egalitarian" => Some(dim_core::sync_engine::ControlMode::Egalitarian),
        "host_only" => Some(dim_core::sync_engine::ControlMode::HostOnly),
        _ => None,
    });

    let info = watch_together
        .create_room_with_options(
            body.media_file_id,
            body.media_id,
            body.media_name,
            user.id.0,
            user.username.clone(),
            user.picture,
            control_mode,
            body.password,
        )
        .await;

    (StatusCode::CREATED, Json(info))
}

pub async fn join_room(
    State(AppState { watch_together, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Path(code): Path<String>,
    body: Option<Json<JoinRoomBody>>,
) -> impl IntoResponse {
    let password = body.and_then(|b| b.0.password);
    match watch_together
        .join_room_with_password(
            &code,
            user.id.0,
            user.username.clone(),
            user.picture,
            password.as_deref(),
        )
        .await
    {
        Ok(info) => Ok(Json(info)),
        Err(e) => Err((
            if e == "Invalid password" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::NOT_FOUND
            },
            e.to_string(),
        )),
    }
}

pub async fn leave_room(
    State(AppState { watch_together, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    match watch_together.leave_room(&code, user.id.0).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err((StatusCode::NOT_FOUND, e.to_string())),
    }
}

pub async fn list_rooms(
    State(AppState { watch_together, .. }): State<AppState>,
) -> impl IntoResponse {
    Json(watch_together.list_rooms().await)
}

pub async fn get_room(
    State(AppState { watch_together, .. }): State<AppState>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    match watch_together.get_room_info(&code).await {
        Some(info) => Ok(Json(info)),
        None => Err((StatusCode::NOT_FOUND, "Room not found".to_string())),
    }
}

pub async fn set_ready(
    State(AppState { watch_together, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Path(code): Path<String>,
    Json(body): Json<ReadyBody>,
) -> impl IntoResponse {
    match watch_together
        .set_ready(&code, user.id.0, body.is_ready)
        .await
    {
        Ok(()) => Ok(StatusCode::OK),
        Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
    }
}

pub async fn transfer_host(
    State(AppState { watch_together, .. }): State<AppState>,
    Extension(user): Extension<User>,
    Path(code): Path<String>,
    Json(body): Json<TransferHostBody>,
) -> impl IntoResponse {
    match watch_together
        .transfer_host(&code, user.id.0, body.to_user_id)
        .await
    {
        Ok(()) => Ok(StatusCode::OK),
        Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
    }
}
