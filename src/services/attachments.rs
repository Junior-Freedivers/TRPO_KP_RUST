//! Вложения: загрузка (с проверкой файла) и выдача на скачивание после проверки прав.

use bytes::Bytes;
use serde_json::json;
use uuid::Uuid;

use crate::audit::{DocumentEvent, document_event};
use crate::dto::AttachmentOut;
use crate::error::{AppError, AppResult};
use crate::models::{ATTACHMENT_SELECT, Attachment, AttachmentType, DocumentStatus, Role, User};
use crate::services::documents::{assert_can_view, find_document};
use crate::state::AppState;
use crate::storage::validation::validate_upload;

/// Файл из multipart-запроса.
pub struct Upload {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Bytes,
    /// тело запроса превысило лимит при чтении
    pub too_large: bool,
}

async fn find_attachment(state: &AppState, id: Uuid) -> AppResult<Attachment> {
    sqlx::query_as(&format!("{ATTACHMENT_SELECT} WHERE f.id = $1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("Файл не найден"))
}

/// Делопроизводитель прикрепляет скан (по умолчанию `source_scan`), исполнитель — отчетные материалы
/// (`execution_report`).
pub async fn upload(
    state: &AppState,
    actor: &User,
    document_id: Uuid,
    file: Upload,
    attachment_type: Option<AttachmentType>,
) -> AppResult<AttachmentOut> {
    let doc = find_document(&state.db, document_id).await?.ok_or_else(|| AppError::not_found("Документ не найден"))?;
    assert_can_view(&state.db, actor, doc.id).await?;
    if doc.status == DocumentStatus::Archived {
        return Err(AppError::conflict("К архивному документу нельзя прикреплять файлы").with_code("DOCUMENT_ARCHIVED"));
    }
    let attachment_type = if actor.role == Role::Executor {
        if !matches!(doc.status, DocumentStatus::InExecution | DocumentStatus::Executed) {
            return Err(AppError::conflict("Отчетные материалы можно прикреплять к документам «на исполнении» и «исполнен»")
                .with_code("INVALID_DOCUMENT_STATE"));
        }
        if attachment_type == Some(AttachmentType::SourceScan) {
            return Err(AppError::forbidden("Исполнитель не может загружать исходный скан документа"));
        }
        attachment_type.unwrap_or(AttachmentType::ExecutionReport)
    } else {
        attachment_type.unwrap_or(AttachmentType::SourceScan)
    };

    let checked = validate_upload(
        file.filename.as_deref(),
        file.content_type.as_deref(),
        &file.data,
        file.too_large,
        state.settings.max_upload_bytes(),
    )?;
    // имя пользователя в ключ хранения не попадает
    let key = format!("documents/{}/{}{}", doc.id, Uuid::new_v4(), checked.extension);
    state.storage.upload(&key, file.data, &checked.content_type).await?;

    let id = Uuid::new_v4();
    let saved: Result<(), sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "INSERT INTO attachments (id, document_id, original_filename, storage_key, content_type, size_bytes, \
             attachment_type, uploaded_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(doc.id)
        .bind(&checked.filename)
        .bind(&key)
        .bind(&checked.content_type)
        .bind(checked.size as i32)
        .bind(attachment_type)
        .bind(actor.id)
        .execute(&mut *tx)
        .await?;
        document_event(&mut tx, doc.id, actor.id, "file_attached", DocumentEvent {
            meta: Some(json!({"attachment_id": id.to_string(), "filename": checked.filename,
                              "type": attachment_type.as_str(), "size_bytes": checked.size})),
            ..Default::default()
        })
        .await?;
        tx.commit().await
    }
    .await;
    if let Err(err) = saved {
        // не оставляем «осиротевший» объект в хранилище
        if let Err(storage_err) = state.storage.delete(&key).await {
            tracing::warn!("Не удалось удалить объект {key} после ошибки записи в БД: {storage_err}");
        }
        return Err(err.into());
    }
    crate::audit!(Some(actor.id), "file_attached", "document_id": doc.id, "attachment_id": id);
    Ok(AttachmentOut::from(&find_attachment(state, id).await?))
}

/// Метаданные файла после проверки прав: роль (DOCUMENT_VIEW) + контекст (исполнитель — только свои).
pub async fn get_for_download(state: &AppState, actor: &User, attachment_id: Uuid) -> AppResult<Attachment> {
    let attachment = find_attachment(state, attachment_id).await?;
    let doc = find_document(&state.db, attachment.document_id)
        .await?
        .ok_or_else(|| AppError::not_found("Документ не найден"))?;
    assert_can_view(&state.db, actor, doc.id).await?;
    if !state.storage.exists(&attachment.storage_key).await? {
        return Err(AppError::not_found("Файл отсутствует в хранилище").with_code("FILE_MISSING"));
    }
    crate::audit!(Some(actor.id), "file_downloaded", "document_id": doc.id, "attachment_id": attachment.id);
    Ok(attachment)
}
