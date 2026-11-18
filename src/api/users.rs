use axum::extract::State;
use axum::http::StatusCode;
use uuid::Uuid;

use crate::auth::{Authorized, can};
use crate::dto::{ExecutorBrief, Page, UserCreate, UserListQuery, UserOut, UserUpdate};
use crate::error::{AppResult, ErrorResponse};
use crate::extract::{Json, Path, Query};
use crate::services::users;
use crate::state::AppState;

/// Регистрация пользователя (admin)
#[utoipa::path(post, path = "/api/v1/users", tag = "users", security(("bearer" = [])), request_body = UserCreate,
    responses((status = 201, body = UserOut), (status = 409, body = ErrorResponse), (status = 422, body = ErrorResponse)))]
pub async fn create_user(
    State(state): State<AppState>,
    auth: Authorized<can::CreateUser>,
    Json(mut data): Json<UserCreate>,
) -> AppResult<(StatusCode, axum::Json<UserOut>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(users::create(&state, &auth.user, data).await?)))
}

/// Список пользователей (admin)
#[utoipa::path(get, path = "/api/v1/users", tag = "users", security(("bearer" = [])), params(UserListQuery),
    responses((status = 200, body = Page<UserOut>), (status = 403, body = ErrorResponse)))]
pub async fn list_users(
    State(state): State<AppState>,
    _auth: Authorized<can::ViewUsers>,
    Query(query): Query<UserListQuery>,
) -> AppResult<axum::Json<Page<UserOut>>> {
    Ok(axum::Json(users::list_page(&state, query).await?))
}

/// Активные исполнители (справочник для назначения резолюций)
///
/// Доступно руководителю и администратору: возвращает только id, ФИО и подразделение.
#[utoipa::path(get, path = "/api/v1/users/executors", tag = "users", security(("bearer" = [])),
    responses((status = 200, body = Vec<ExecutorBrief>), (status = 403, body = ErrorResponse)))]
pub async fn list_executors(
    State(state): State<AppState>,
    _auth: Authorized<can::ListExecutors>,
) -> AppResult<axum::Json<Vec<ExecutorBrief>>> {
    Ok(axum::Json(users::list_executors(&state).await?))
}

/// Профиль пользователя (admin)
#[utoipa::path(get, path = "/api/v1/users/{user_id}", tag = "users", security(("bearer" = [])),
    params(("user_id" = Uuid, Path)),
    responses((status = 200, body = UserOut), (status = 404, body = ErrorResponse)))]
pub async fn get_user(
    State(state): State<AppState>,
    _auth: Authorized<can::ViewUsers>,
    Path(user_id): Path<Uuid>,
) -> AppResult<axum::Json<UserOut>> {
    Ok(axum::Json(users::get(&state, user_id).await?))
}

/// Редактирование пользователя (admin)
#[utoipa::path(patch, path = "/api/v1/users/{user_id}", tag = "users", security(("bearer" = [])),
    params(("user_id" = Uuid, Path)), request_body = UserUpdate,
    responses((status = 200, body = UserOut), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn update_user(
    State(state): State<AppState>,
    auth: Authorized<can::EditUser>,
    Path(user_id): Path<Uuid>,
    Json(mut data): Json<UserUpdate>,
) -> AppResult<axum::Json<UserOut>> {
    data.validate()?;
    Ok(axum::Json(users::update(&state, &auth.user, user_id, data).await?))
}

/// Деактивация пользователя без физического удаления (admin)
#[utoipa::path(post, path = "/api/v1/users/{user_id}/deactivate", tag = "users", security(("bearer" = [])),
    params(("user_id" = Uuid, Path)),
    responses((status = 200, body = UserOut), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn deactivate_user(
    State(state): State<AppState>,
    auth: Authorized<can::DeactivateUser>,
    Path(user_id): Path<Uuid>,
) -> AppResult<axum::Json<UserOut>> {
    Ok(axum::Json(users::deactivate(&state, &auth.user, user_id).await?))
}
