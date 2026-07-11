use crate::AppState;
use axum::extract::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Extension;

use dim_core::settings;
use dim_core::settings::{set_global_settings, GlobalSettings};
use dim_database::user::UpdateableUser;
use dim_database::user::User;
use dim_database::user::UserSettings;
use dim_database::DatabaseError;

use super::auth::AuthError;

pub async fn get_user_settings(
    Extension(user): Extension<User>,
    State(AppState { conn, .. }): State<AppState>,
) -> Result<Response, AuthError> {
    let mut tx = conn.read().begin().await.map_err(DatabaseError::from)?;
    let prefs = User::get_by_id(&mut tx, user.id).await?.prefs;
    tracing::info!(
        user_id = ?user.id,
        default_video_quality = ?prefs.default_video_quality,
        "GET user settings"
    );
    Ok(axum::response::Json(&prefs).into_response())
}

pub async fn post_user_settings(
    Extension(user): Extension<User>,
    State(AppState { conn, .. }): State<AppState>,
    Json(new_settings): Json<UserSettings>,
) -> Result<Response, AuthError> {
    tracing::info!(
        user_id = ?user.id,
        default_video_quality = ?new_settings.default_video_quality,
        "POST user settings (saving)"
    );

    let mut lock = conn.writer().lock_owned().await;
    let mut tx = dim_database::write_tx(&mut lock)
        .await
        .map_err(DatabaseError::from)?;
    let update_user = UpdateableUser {
        prefs: Some(new_settings.clone()),
    };

    let rows = update_user.update(&mut tx, user.id).await?;
    tracing::info!(rows_affected = rows, "UPDATE executed");

    tx.commit().await.map_err(DatabaseError::from)?;
    drop(lock);

    // Verify: re-read from the read pool to confirm persistence
    let mut verify_tx = conn.read().begin().await.map_err(DatabaseError::from)?;
    let verify_user = User::get_by_id(&mut verify_tx, user.id).await?;
    tracing::info!(
        default_video_quality = ?verify_user.prefs.default_video_quality,
        "POST verify: re-read from DB after commit"
    );

    Ok(axum::response::Json(&new_settings).into_response())
}

fn get_global_settings() -> GlobalSettings {
    let mut global_settings: GlobalSettings = settings::get_global_settings();
    let build_time = env!("BUILD_TIME");
    let arch = env!("BUILD_ARCH");
    let profile = env!("BUILD_PROFILE");
    let cpu_features = env!("BUILD_CPU_FEATURES");

    global_settings.version = format!(
        "{arch} {profile} | {build_time} | cpu: {cpu_features}"
    );
    global_settings
}

// TODO: Hide secret key.
pub async fn http_get_global_settings() -> Result<Response, AuthError> {
    Ok(axum::response::Json(&get_global_settings()).into_response())
}

pub async fn get_detected_devices() -> impl IntoResponse {
    axum::response::Json(nightfall::profiles::detect_devices())
}

// TODO: Disallow setting secret key over http.
pub async fn http_set_global_settings(
    Extension(user): Extension<User>,
    Json(new_settings): Json<GlobalSettings>,
) -> Result<Response, AuthError> {
    if user.has_role("owner") {
        set_global_settings(new_settings).unwrap();
        return Ok(Json(&get_global_settings()).into_response());
    }

    Err(AuthError::InvalidCredentials)
}
