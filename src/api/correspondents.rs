use axum::extract::State;
use axum::http::StatusCode;
use uuid::Uuid;

use crate::auth::{Authorized, can};
use crate::dto::{CorrespondentCreate, CorrespondentListQuery, CorrespondentOut, CorrespondentUpdate, Page};
use crate::error::{AppResult, ErrorResponse};
use crate::extract::{Json, Path, Query};
use crate::services::correspondents as svc;
use crate::state::AppState;

/// Создать корреспондента
#[utoipa::path(post, path = "/api/v1/correspondents", tag = "correspondents", security(("bearer" = [])),
    request_body = CorrespondentCreate,
    responses((status = 201, body = CorrespondentOut), (status = 422, body = ErrorResponse)))]
pub async fn create_correspondent(
    State(state): State<AppState>,
    auth: Authorized<can::CreateCorrespondent>,
    Json(mut data): Json<CorrespondentCreate>,
) -> AppResult<(StatusCode, axum::Json<CorrespondentOut>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(svc::create(&state, &auth.user, data).await?)))
}

/// Список корреспондентов
#[utoipa::path(get, path = "/api/v1/correspondents", tag = "correspondents", security(("bearer" = [])),
    params(CorrespondentListQuery),
    responses((status = 200, body = Page<CorrespondentOut>), (status = 403, body = ErrorResponse)))]
pub async fn list_correspondents(
    State(state): State<AppState>,
    _auth: Authorized<can::ViewCorrespondents>,
    Query(query): Query<CorrespondentListQuery>,
) -> AppResult<axum::Json<Page<CorrespondentOut>>> {
    Ok(axum::Json(svc::list_page(&state, query).await?))
}

/// Карточка корреспондента
#[utoipa::path(get, path = "/api/v1/correspondents/{correspondent_id}", tag = "correspondents",
    security(("bearer" = [])), params(("correspondent_id" = Uuid, Path)),
    responses((status = 200, body = CorrespondentOut), (status = 404, body = ErrorResponse)))]
pub async fn get_correspondent(
    State(state): State<AppState>,
    _auth: Authorized<can::ViewCorrespondents>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::Json<CorrespondentOut>> {
    Ok(axum::Json(svc::get(&state, id).await?))
}

/// Редактировать корреспондента
#[utoipa::path(patch, path = "/api/v1/correspondents/{correspondent_id}", tag = "correspondents",
    security(("bearer" = [])), params(("correspondent_id" = Uuid, Path)), request_body = CorrespondentUpdate,
    responses((status = 200, body = CorrespondentOut), (status = 404, body = ErrorResponse), (status = 422, body = ErrorResponse)))]
pub async fn update_correspondent(
    State(state): State<AppState>,
    auth: Authorized<can::EditCorrespondent>,
    Path(id): Path<Uuid>,
    Json(mut data): Json<CorrespondentUpdate>,
) -> AppResult<axum::Json<CorrespondentOut>> {
    data.validate()?;
    Ok(axum::Json(svc::update(&state, &auth.user, id, data).await?))
}
