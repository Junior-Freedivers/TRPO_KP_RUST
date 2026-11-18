use axum::extract::State;
use axum::http::StatusCode;
use uuid::Uuid;

use crate::auth::{Authorized, CurrentUser, can};
use crate::dto::{
    DepartmentOut, DocumentTypeCreate, DocumentTypeOut, DocumentTypeUpdate, NameCreate, NameUpdate, PositionOut,
};
use crate::error::{AppResult, ErrorResponse};
use crate::extract::{Json, Path};
use crate::services::dictionaries as svc;
use crate::state::AppState;

type Manage = Authorized<can::ManageDictionaries>;

/// Список подразделений (admin)
#[utoipa::path(get, path = "/api/v1/departments", tag = "departments", security(("bearer" = [])),
    responses((status = 200, body = Vec<DepartmentOut>), (status = 403, body = ErrorResponse)))]
pub async fn list_departments(State(state): State<AppState>, _auth: Manage) -> AppResult<axum::Json<Vec<DepartmentOut>>> {
    Ok(axum::Json(svc::list_departments(&state).await?))
}

/// Создать подразделение (admin)
#[utoipa::path(post, path = "/api/v1/departments", tag = "departments", security(("bearer" = [])),
    request_body = NameCreate,
    responses((status = 201, body = DepartmentOut), (status = 409, body = ErrorResponse)))]
pub async fn create_department(
    State(state): State<AppState>,
    auth: Manage,
    Json(mut data): Json<NameCreate>,
) -> AppResult<(StatusCode, axum::Json<DepartmentOut>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(svc::create_department(&state, &auth.user, &data.name).await?)))
}

/// Изменить подразделение (admin)
#[utoipa::path(patch, path = "/api/v1/departments/{item_id}", tag = "departments", security(("bearer" = [])),
    params(("item_id" = Uuid, Path)), request_body = NameUpdate,
    responses((status = 200, body = DepartmentOut), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn update_department(
    State(state): State<AppState>,
    auth: Manage,
    Path(item_id): Path<Uuid>,
    Json(mut data): Json<NameUpdate>,
) -> AppResult<axum::Json<DepartmentOut>> {
    data.validate()?;
    Ok(axum::Json(svc::update_department(&state, &auth.user, item_id, data.name).await?))
}

/// Список должностей (admin)
#[utoipa::path(get, path = "/api/v1/positions", tag = "positions", security(("bearer" = [])),
    responses((status = 200, body = Vec<PositionOut>), (status = 403, body = ErrorResponse)))]
pub async fn list_positions(State(state): State<AppState>, _auth: Manage) -> AppResult<axum::Json<Vec<PositionOut>>> {
    Ok(axum::Json(svc::list_positions(&state).await?))
}

/// Создать должность (admin)
#[utoipa::path(post, path = "/api/v1/positions", tag = "positions", security(("bearer" = [])),
    request_body = NameCreate,
    responses((status = 201, body = PositionOut), (status = 409, body = ErrorResponse)))]
pub async fn create_position(
    State(state): State<AppState>,
    auth: Manage,
    Json(mut data): Json<NameCreate>,
) -> AppResult<(StatusCode, axum::Json<PositionOut>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(svc::create_position(&state, &auth.user, &data.name).await?)))
}

/// Изменить должность (admin)
#[utoipa::path(patch, path = "/api/v1/positions/{item_id}", tag = "positions", security(("bearer" = [])),
    params(("item_id" = Uuid, Path)), request_body = NameUpdate,
    responses((status = 200, body = PositionOut), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn update_position(
    State(state): State<AppState>,
    auth: Manage,
    Path(item_id): Path<Uuid>,
    Json(mut data): Json<NameUpdate>,
) -> AppResult<axum::Json<PositionOut>> {
    data.validate()?;
    Ok(axum::Json(svc::update_position(&state, &auth.user, item_id, data.name).await?))
}

/// Типы документов (любой авторизованный)
#[utoipa::path(get, path = "/api/v1/document-types", tag = "document-types", security(("bearer" = [])),
    responses((status = 200, body = Vec<DocumentTypeOut>), (status = 401, body = ErrorResponse)))]
pub async fn list_document_types(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> AppResult<axum::Json<Vec<DocumentTypeOut>>> {
    Ok(axum::Json(svc::list_types(&state).await?))
}

/// Создать тип документа (admin)
#[utoipa::path(post, path = "/api/v1/document-types", tag = "document-types", security(("bearer" = [])),
    request_body = DocumentTypeCreate,
    responses((status = 201, body = DocumentTypeOut), (status = 409, body = ErrorResponse)))]
pub async fn create_document_type(
    State(state): State<AppState>,
    auth: Manage,
    Json(mut data): Json<DocumentTypeCreate>,
) -> AppResult<(StatusCode, axum::Json<DocumentTypeOut>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(svc::create_type(&state, &auth.user, &data.name, data.is_active).await?)))
}

/// Изменить тип документа (admin)
#[utoipa::path(patch, path = "/api/v1/document-types/{item_id}", tag = "document-types", security(("bearer" = [])),
    params(("item_id" = Uuid, Path)), request_body = DocumentTypeUpdate,
    responses((status = 200, body = DocumentTypeOut), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn update_document_type(
    State(state): State<AppState>,
    auth: Manage,
    Path(item_id): Path<Uuid>,
    Json(mut data): Json<DocumentTypeUpdate>,
) -> AppResult<axum::Json<DocumentTypeOut>> {
    data.validate()?;
    Ok(axum::Json(svc::update_type(&state, &auth.user, item_id, data.name, data.is_active).await?))
}
