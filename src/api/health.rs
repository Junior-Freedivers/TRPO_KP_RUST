use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;

use crate::dto::HealthResponse;
use crate::state::AppState;

const TIMEOUT: Duration = Duration::from_secs(3);

async fn check_db(state: &AppState) -> &'static str {
    match tokio::time::timeout(TIMEOUT, sqlx::query("SELECT 1").execute(&state.db)).await {
        Ok(Ok(_)) => "ok",
        Ok(Err(err)) => {
            tracing::warn!("Health: БД недоступна: {err}");
            "unavailable"
        }
        Err(_) => {
            tracing::warn!("Health: БД не ответила за {TIMEOUT:?}");
            "unavailable"
        }
    }
}

async fn check_storage(state: &AppState) -> &'static str {
    match tokio::time::timeout(TIMEOUT, state.storage.check()).await {
        Ok(Ok(())) => "ok",
        Ok(Err(err)) => {
            tracing::warn!("Health: файловое хранилище недоступно: {err}");
            "unavailable"
        }
        Err(_) => {
            tracing::warn!("Health: файловое хранилище не ответило за {TIMEOUT:?}");
            "unavailable"
        }
    }
}

/// Проверка БД, файлового хранилища (SeaweedFS) и доступности сервиса
#[utoipa::path(get, path = "/health", tag = "health",
    responses(
        (status = 200, body = HealthResponse),
        (status = 503, body = HealthResponse, description = "Одна из зависимостей недоступна"),
    ))]
pub async fn health(State(state): State<AppState>) -> (StatusCode, axum::Json<HealthResponse>) {
    let (database, storage) = tokio::join!(check_db(&state), check_storage(&state));
    let ok = database == "ok" && storage == "ok";
    let body = HealthResponse {
        status: if ok { "ok" } else { "degraded" }.into(),
        database: database.into(),
        storage: storage.into(),
    };
    (if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }, axum::Json(body))
}
