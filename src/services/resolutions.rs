//! Резолюции руководителя: создание, просмотр, редактирование до завершения исполнения.

use serde_json::{Map, json};
use sqlx::{PgExecutor, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::audit::{DocumentEvent, document_event};
use crate::domain::workflow::RESOLUTION_OPEN_STATUSES;
use crate::dto::{Jsonable, ResolutionCreate, ResolutionOut, ResolutionUpdate};
use crate::error::{AppError, AppResult};
use crate::models::{DocumentStatus, RESOLUTION_SELECT, Resolution, ResolutionStatus, Role, User};
use crate::services::documents::{assert_can_view, find_document, load_resolutions, lock_document};
use crate::services::users::find_user;
use crate::state::AppState;

async fn find_resolution<'e>(db: impl PgExecutor<'e>, id: Uuid) -> Result<Option<Resolution>, sqlx::Error> {
    sqlx::query_as(&format!("{RESOLUTION_SELECT} WHERE r.id = $1")).bind(id).fetch_optional(db).await
}

async fn fresh(state: &AppState, id: Uuid) -> AppResult<ResolutionOut> {
    let resolution = find_resolution(&state.db, id).await?.ok_or_else(|| AppError::not_found("Резолюция не найдена"))?;
    Ok(ResolutionOut::from(&resolution))
}

async fn require_executor<'e>(db: impl PgExecutor<'e>, executor_id: Uuid) -> AppResult<User> {
    let user = find_user(db, executor_id)
        .await?
        .ok_or_else(|| AppError::validation("Исполнитель не найден").with_field("assigned_executor_id"))?;
    if user.role != Role::Executor || !user.is_active {
        return Err(AppError::validation("Назначить можно только активного пользователя с ролью «исполнитель»")
            .with_field("assigned_executor_id"));
    }
    Ok(user)
}

fn deadline_error() -> AppError {
    AppError::validation("Срок резолюции не может быть раньше даты регистрации документа").with_field("deadline")
}

/// При первой резолюции документ автоматически переходит «на рассмотрении» → «на исполнении».
pub async fn create(state: &AppState, actor: &User, document_id: Uuid, data: ResolutionCreate) -> AppResult<ResolutionOut> {
    let mut tx = state.db.begin().await?;
    if !lock_document(&mut tx, document_id).await? {
        return Err(AppError::not_found("Документ не найден"));
    }
    let doc = find_document(&mut *tx, document_id).await?.ok_or_else(|| AppError::not_found("Документ не найден"))?;
    if !RESOLUTION_OPEN_STATUSES.contains(&doc.status) {
        return Err(AppError::conflict("Резолюцию можно создать только для документа «на рассмотрении» или «на исполнении»")
            .with_code("INVALID_DOCUMENT_STATE"));
    }
    let executor = require_executor(&mut *tx, data.assigned_executor_id).await?;
    if data.deadline < doc.registration_date {
        return Err(deadline_error());
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO resolutions (id, document_id, text, author_id, assigned_executor_id, deadline, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(doc.id)
    .bind(data.text.trim())
    .bind(actor.id)
    .bind(executor.id)
    .bind(data.deadline)
    .bind(ResolutionStatus::InProgress)
    .execute(&mut *tx)
    .await?;
    document_event(&mut tx, doc.id, actor.id, "resolution_created", DocumentEvent {
        meta: Some(json!({"resolution_id": id.to_string(), "executor_id": executor.id.to_string(),
                          "deadline": data.deadline.to_json()})),
        ..Default::default()
    })
    .await?;
    if doc.status == DocumentStatus::UnderReview {
        sqlx::query("UPDATE incoming_documents SET status = $1, updated_at = now() WHERE id = $2")
            .bind(DocumentStatus::InExecution)
            .bind(doc.id)
            .execute(&mut *tx)
            .await?;
        document_event(&mut tx, doc.id, actor.id, "status_changed", DocumentEvent {
            old_status: Some(DocumentStatus::UnderReview),
            new_status: Some(DocumentStatus::InExecution),
            comment: Some("Создана резолюция"),
            meta: None,
        })
        .await?;
    }
    tx.commit().await?;
    crate::audit!(Some(actor.id), "resolution_created", "document_id": doc.id, "resolution_id": id);
    fresh(state, id).await
}

pub async fn list_for_document(state: &AppState, actor: &User, document_id: Uuid) -> AppResult<Vec<ResolutionOut>> {
    let doc = find_document(&state.db, document_id).await?.ok_or_else(|| AppError::not_found("Документ не найден"))?;
    assert_can_view(&state.db, actor, doc.id).await?;
    let scope = (actor.role == Role::Executor).then_some(actor.id);
    Ok(load_resolutions(&state.db, doc.id, scope).await?.iter().map(ResolutionOut::from).collect())
}

pub async fn update(state: &AppState, actor: &User, resolution_id: Uuid, data: ResolutionUpdate) -> AppResult<ResolutionOut> {
    let resolution = find_resolution(&state.db, resolution_id)
        .await?
        .ok_or_else(|| AppError::not_found("Резолюция не найдена"))?;
    let mut tx = state.db.begin().await?;
    if !lock_document(&mut tx, resolution.document_id).await? {
        return Err(AppError::not_found("Документ не найден"));
    }
    let resolution = find_resolution(&mut *tx, resolution_id)
        .await?
        .ok_or_else(|| AppError::not_found("Резолюция не найдена"))?;
    let doc = find_document(&mut *tx, resolution.document_id)
        .await?
        .ok_or_else(|| AppError::not_found("Документ не найден"))?;
    if resolution.status != ResolutionStatus::InProgress || !RESOLUTION_OPEN_STATUSES.contains(&doc.status) {
        return Err(AppError::conflict("Резолюцию нельзя изменить: процесс исполнения завершен")
            .with_code("RESOLUTION_LOCKED"));
    }

    let text = data.text.required("text")?;
    let executor_id = data.assigned_executor_id.required("assigned_executor_id")?;
    let deadline = data.deadline.required("deadline")?;
    if let Some(executor) = executor_id
        && executor != resolution.assigned_executor_id
    {
        require_executor(&mut *tx, executor).await?;
    }
    if let Some(deadline) = deadline
        && deadline < doc.registration_date
    {
        return Err(deadline_error());
    }

    let mut diff = Map::new();
    let mut qb = QueryBuilder::<Postgres>::new("UPDATE resolutions SET updated_at = now()");
    let mut track = |name: &str, old: serde_json::Value, new: serde_json::Value| {
        if old != new {
            diff.insert(name.to_string(), json!({"old": old, "new": new}));
            true
        } else {
            false
        }
    };
    if let Some(v) = text
        && track("text", resolution.text.to_json(), v.to_json())
    {
        qb.push(", text = ").push_bind(v);
    }
    if let Some(v) = executor_id
        && track("assigned_executor_id", resolution.assigned_executor_id.to_json(), v.to_json())
    {
        qb.push(", assigned_executor_id = ").push_bind(v);
    }
    if let Some(v) = deadline
        && track("deadline", resolution.deadline.to_json(), v.to_json())
    {
        qb.push(", deadline = ").push_bind(v);
    }
    if !diff.is_empty() {
        qb.push(" WHERE id = ").push_bind(resolution.id);
        qb.build().execute(&mut *tx).await?;
        let mut fields: Vec<String> = diff.keys().cloned().collect();
        fields.sort();
        document_event(&mut tx, doc.id, actor.id, "resolution_updated", DocumentEvent {
            meta: Some(json!({"resolution_id": resolution.id.to_string(), "changes": diff})),
            ..Default::default()
        })
        .await?;
        tx.commit().await?;
        crate::audit!(Some(actor.id), "resolution_updated", "resolution_id": resolution.id, "fields": fields);
    }
    fresh(state, resolution_id).await
}
