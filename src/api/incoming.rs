use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use bytes::{Bytes, BytesMut};
use uuid::Uuid;

use crate::auth::{Authorized, can};
use crate::dto::{
    AttachmentOut, DocumentCreate, DocumentDetail, DocumentFilters, DocumentOut, DocumentUpdate, HistoryOut, Page,
    ResolutionCreate, ResolutionOut, ResolutionUpdate, StatusChangeRequest,
};
use crate::error::{AppError, AppResult, ErrorResponse};
use crate::extract::{Json, Path, Query};
use crate::models::AttachmentType;
use crate::services::attachments::{self, Upload};
use crate::services::{documents, resolutions};
use crate::state::AppState;

/// Регистрация карточки входящего документа (clerk)
///
/// Регистрационный номер и дата регистрации присваиваются автоматически (атомарно).
/// Файл скана прикрепляется следующим запросом `POST /incoming/{id}/files`.
#[utoipa::path(post, path = "/api/v1/incoming", tag = "incoming", security(("bearer" = [])),
    request_body = DocumentCreate,
    responses((status = 201, body = DocumentDetail), (status = 403, body = ErrorResponse), (status = 422, body = ErrorResponse)))]
pub async fn register_document(
    State(state): State<AppState>,
    auth: Authorized<can::RegisterDocument>,
    Json(data): Json<DocumentCreate>,
) -> AppResult<(StatusCode, axum::Json<DocumentDetail>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(documents::register(&state, &auth.user, data).await?)))
}

/// Поиск, фильтрация и список карточек
///
/// Исполнитель видит только документы со своими резолюциями и не может использовать поиск/фильтры.
#[utoipa::path(get, path = "/api/v1/incoming", tag = "incoming", security(("bearer" = [])), params(DocumentFilters),
    responses((status = 200, body = Page<DocumentOut>), (status = 403, body = ErrorResponse), (status = 422, body = ErrorResponse)))]
pub async fn list_documents(
    State(state): State<AppState>,
    auth: Authorized<can::ViewDocuments>,
    Query(filters): Query<DocumentFilters>,
) -> AppResult<axum::Json<Page<DocumentOut>>> {
    Ok(axum::Json(documents::search(&state, &auth.user, filters).await?))
}

/// Карточка по регистрационному номеру
#[utoipa::path(get, path = "/api/v1/incoming/by-registration-number/{registration_number}", tag = "incoming",
    security(("bearer" = [])), params(("registration_number" = String, Path)),
    responses((status = 200, body = DocumentDetail), (status = 404, body = ErrorResponse)))]
pub async fn get_by_number(
    State(state): State<AppState>,
    auth: Authorized<can::ViewDocuments>,
    Path(number): Path<String>,
) -> AppResult<axum::Json<DocumentDetail>> {
    Ok(axum::Json(documents::get_by_number(&state, &auth.user, &number).await?))
}

/// Карточка документа: реквизиты, резолюции, вложения, история
#[utoipa::path(get, path = "/api/v1/incoming/{document_id}", tag = "incoming", security(("bearer" = [])),
    params(("document_id" = Uuid, Path)),
    responses((status = 200, body = DocumentDetail), (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse)))]
pub async fn get_document(
    State(state): State<AppState>,
    auth: Authorized<can::ViewDocuments>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::Json<DocumentDetail>> {
    Ok(axum::Json(documents::get_detail(&state, &auth.user, id).await?))
}

/// Редактирование карточки до архивирования (clerk)
#[utoipa::path(patch, path = "/api/v1/incoming/{document_id}", tag = "incoming", security(("bearer" = [])),
    params(("document_id" = Uuid, Path)), request_body = DocumentUpdate,
    responses((status = 200, body = DocumentDetail), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn update_document(
    State(state): State<AppState>,
    auth: Authorized<can::EditDocument>,
    Path(id): Path<Uuid>,
    Json(data): Json<DocumentUpdate>,
) -> AppResult<axum::Json<DocumentDetail>> {
    data.validate()?;
    Ok(axum::Json(documents::update(&state, &auth.user, id, data).await?))
}

/// Удаление карточки (только admin)
///
/// Исключительный случай: ошибочная регистрация или дубликат. Делопроизводитель аннулирует запись
/// сменой статуса, а не удалением.
#[utoipa::path(delete, path = "/api/v1/incoming/{document_id}", tag = "incoming", security(("bearer" = [])),
    params(("document_id" = Uuid, Path)),
    responses((status = 204, description = "Удалено"), (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse)))]
pub async fn delete_document(
    State(state): State<AppState>,
    auth: Authorized<can::DeleteDocument>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    documents::delete(&state, &auth.user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// История действий по карточке
#[utoipa::path(get, path = "/api/v1/incoming/{document_id}/history", tag = "incoming", security(("bearer" = [])),
    params(("document_id" = Uuid, Path)),
    responses((status = 200, body = Vec<HistoryOut>), (status = 404, body = ErrorResponse)))]
pub async fn get_history(
    State(state): State<AppState>,
    auth: Authorized<can::ViewDocuments>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::Json<Vec<HistoryOut>>> {
    Ok(axum::Json(documents::history(&state, &auth.user, id).await?))
}

/// Смена статуса карточки (с записью в историю)
///
/// Допустимость перехода и роль проверяются централизованной машиной состояний; недопустимый переход — 409.
#[utoipa::path(post, path = "/api/v1/incoming/{document_id}/status", tag = "incoming", security(("bearer" = [])),
    params(("document_id" = Uuid, Path)), request_body = StatusChangeRequest,
    responses((status = 200, body = DocumentDetail), (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn change_status(
    State(state): State<AppState>,
    auth: Authorized<can::ChangeStatus>,
    Path(id): Path<Uuid>,
    Json(data): Json<StatusChangeRequest>,
) -> AppResult<axum::Json<DocumentDetail>> {
    data.validate()?;
    Ok(axum::Json(documents::change_status(&state, &auth.user, id, data).await?))
}

/// Создание резолюции (manager)
///
/// При первой резолюции документ автоматически переходит «на рассмотрении» → «на исполнении».
#[utoipa::path(post, path = "/api/v1/incoming/{document_id}/resolutions", tag = "incoming",
    security(("bearer" = [])), params(("document_id" = Uuid, Path)), request_body = ResolutionCreate,
    responses((status = 201, body = ResolutionOut), (status = 409, body = ErrorResponse), (status = 422, body = ErrorResponse)))]
pub async fn create_resolution(
    State(state): State<AppState>,
    auth: Authorized<can::CreateResolution>,
    Path(id): Path<Uuid>,
    Json(data): Json<ResolutionCreate>,
) -> AppResult<(StatusCode, axum::Json<ResolutionOut>)> {
    data.validate()?;
    Ok((StatusCode::CREATED, axum::Json(resolutions::create(&state, &auth.user, id, data).await?)))
}

/// Резолюции по документу
#[utoipa::path(get, path = "/api/v1/incoming/{document_id}/resolutions", tag = "incoming",
    security(("bearer" = [])), params(("document_id" = Uuid, Path)),
    responses((status = 200, body = Vec<ResolutionOut>), (status = 404, body = ErrorResponse)))]
pub async fn list_resolutions(
    State(state): State<AppState>,
    auth: Authorized<can::ViewResolutions>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::Json<Vec<ResolutionOut>>> {
    Ok(axum::Json(resolutions::list_for_document(&state, &auth.user, id).await?))
}

/// Редактирование резолюции до завершения процесса (manager)
#[utoipa::path(patch, path = "/api/v1/resolutions/{resolution_id}", tag = "resolutions", security(("bearer" = [])),
    params(("resolution_id" = Uuid, Path)), request_body = ResolutionUpdate,
    responses((status = 200, body = ResolutionOut), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse)))]
pub async fn update_resolution(
    State(state): State<AppState>,
    auth: Authorized<can::EditResolution>,
    Path(id): Path<Uuid>,
    Json(data): Json<ResolutionUpdate>,
) -> AppResult<axum::Json<ResolutionOut>> {
    data.validate()?;
    Ok(axum::Json(resolutions::update(&state, &auth.user, id, data).await?))
}

/// Прикрепление файла (PDF, JPG, PNG, DOCX) к карточке
///
/// Делопроизводитель прикрепляет скан (по умолчанию `source_scan`), исполнитель — отчетные материалы
/// (`execution_report`). Размер ограничен MAX_UPLOAD_SIZE_MB.
#[utoipa::path(post, path = "/api/v1/incoming/{document_id}/files", tag = "incoming", security(("bearer" = [])),
    params(("document_id" = Uuid, Path)),
    request_body(content = UploadForm, content_type = "multipart/form-data"),
    responses((status = 201, body = AttachmentOut), (status = 413, body = ErrorResponse), (status = 422, body = ErrorResponse)))]
pub async fn upload_file(
    State(state): State<AppState>,
    auth: Authorized<can::AttachFile>,
    Path(id): Path<Uuid>,
    multipart: Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> AppResult<(StatusCode, axum::Json<AttachmentOut>)> {
    let mut multipart =
        multipart.map_err(|e| AppError::validation(format!("Ожидается multipart/form-data: {}", e.body_text())))?;
    let max = state.settings.max_upload_bytes();
    let mut upload: Option<Upload> = None;
    let mut attachment_type: Option<AttachmentType> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => {
                // превышен лимит тела запроса (или оборван поток) — дальнейшее чтение невозможно
                if let Some(file) = upload.as_mut() {
                    file.too_large = true;
                } else if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    return Err(AppError::too_large(format!("Размер файла превышает {} МБ", max / (1024 * 1024)))
                        .with_field("file"));
                } else {
                    return Err(AppError::validation(format!("Некорректный multipart-запрос: {}", err.body_text())));
                }
                break;
            }
        };
        match field.name() {
            Some("file") => {
                let filename = field.file_name().map(str::to_string);
                let content_type = field.content_type().map(str::to_string);
                let (data, too_large) = read_limited(field, max).await;
                upload = Some(Upload { filename, content_type, data, too_large });
                if too_large {
                    break;
                }
            }
            Some("attachment_type") => {
                let value = field.text().await.unwrap_or_default();
                if !value.is_empty() {
                    attachment_type = Some(AttachmentType::parse(&value).ok_or_else(|| {
                        AppError::invalid_field(
                            "attachment_type",
                            "допустимые значения: source_scan, execution_report, other",
                        )
                    })?);
                }
            }
            _ => {}
        }
    }
    let upload = upload.ok_or_else(|| AppError::invalid_field("file", "Обязательное поле"))?;
    let out = attachments::upload(&state, &auth.user, id, upload, attachment_type).await?;
    Ok((StatusCode::CREATED, axum::Json(out)))
}

/// Читает поле не более `max + 1` байт (чтобы обнаружить превышение лимита).
async fn read_limited(mut field: axum::extract::multipart::Field<'_>, max: usize) -> (Bytes, bool) {
    let mut buffer = BytesMut::new();
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                buffer.extend_from_slice(&chunk);
                if buffer.len() > max {
                    return (buffer.freeze(), true);
                }
            }
            Ok(None) => return (buffer.freeze(), false),
            Err(_) => return (buffer.freeze(), true),
        }
    }
}

#[allow(unused_imports)]
use crate::dto::UploadForm;
