//! Журналирование действий пользователей.
//!
//! * История карточки (document_history) — в БД, в той же транзакции, что и само действие.
//! * Прочие операции (вход, отчеты, выгрузки, удаление) — структурированные записи в stdout.

use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::DocumentStatus;

/// Структурированная запись журнала операций: `action=… actor=… details={…}`.
pub fn log_action(actor: Option<Uuid>, action: &str, details: Value) {
    let actor = actor.map_or_else(|| "None".to_string(), |id| id.to_string());
    tracing::info!(target: "app.audit", "action={action} actor={actor} details={details}");
}

#[macro_export]
macro_rules! audit {
    ($actor:expr, $action:expr $(, $key:literal : $value:expr)* $(,)?) => {
        $crate::audit::log_action($actor, $action, serde_json::json!({ $($key: $value),* }))
    };
}

/// Событие истории карточки документа.
#[derive(Debug, Default)]
pub struct DocumentEvent<'a> {
    pub old_status: Option<DocumentStatus>,
    pub new_status: Option<DocumentStatus>,
    pub comment: Option<&'a str>,
    pub meta: Option<Value>,
}

pub async fn document_event(
    conn: &mut PgConnection,
    document_id: Uuid,
    user_id: Uuid,
    action: &str,
    event: DocumentEvent<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO document_history (id, document_id, user_id, action, old_status, new_status, comment, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(user_id)
    .bind(action)
    .bind(event.old_status)
    .bind(event.new_status)
    .bind(event.comment)
    .bind(event.meta)
    .execute(conn)
    .await?;
    Ok(())
}
