//! Доменные перечисления и строки таблиц БД.
//!
//! Перечисления хранятся в БД как VARCHAR (без нативного PG ENUM) — так же, как в исходной схеме.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Перечисление, которое (де)сериализуется и хранится в БД как фиксированная строка.
macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $value)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self { $($name::$variant => $value),+ }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value { $($value => Some($name::$variant),)+ _ => None }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl sqlx::Type<sqlx::Postgres> for $name {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <String as sqlx::Type<sqlx::Postgres>>::type_info()
            }

            fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <&str as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.as_str(), buf)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $name {
            fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
                let raw = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
                $name::parse(raw).ok_or_else(|| format!("недопустимое значение {}: {raw}", stringify!($name)).into())
            }
        }
    };
}

str_enum! {
    /// Роль пользователя.
    Role {
        /// делопроизводитель
        Clerk => "clerk",
        /// руководитель
        Manager => "manager",
        /// исполнитель
        Executor => "executor",
        /// администратор
        Admin => "admin",
    }
}

str_enum! {
    /// Статус карточки входящего документа.
    DocumentStatus {
        Registered => "зарегистрирован",
        UnderReview => "на рассмотрении",
        InExecution => "на исполнении",
        Executed => "исполнен",
        RemovedFromControl => "снят с контроля",
        Archived => "в архиве",
    }
}

str_enum! {
    /// Статус резолюции.
    ResolutionStatus {
        InProgress => "на исполнении",
        Executed => "исполнена",
    }
}

str_enum! {
    /// Тип вложения.
    AttachmentType {
        SourceScan => "source_scan",
        ExecutionReport => "execution_report",
        Other => "other",
    }
}

str_enum! {
    /// Состояние контроля срока исполнения.
    DeadlineState {
        /// срок не близок
        Ok => "ok",
        /// срок наступает в ближайшие DEADLINE_WARNING_DAYS дней
        Warning => "warning",
        /// срок истек, документ не исполнен
        Overdue => "overdue",
        /// документ исполнен / снят с контроля / в архиве
        Closed => "closed",
    }
}

str_enum! {
    /// Тип PDF-отчета.
    ReportType {
        /// по документам
        Documents => "documents",
        /// по исполнителям
        Executors => "executors",
        /// по срокам исполнения
        Deadlines => "deadlines",
    }
}

// ------------------------------------------------------------------------------------------------
// Строки таблиц (с полями связанных справочников, полученными через JOIN)
// ------------------------------------------------------------------------------------------------

pub const USER_SELECT: &str = "SELECT u.id, u.full_name, u.login, u.email, u.password_hash, u.role, \
    u.department_id, d.name AS department_name, u.position_id, p.name AS position_name, u.is_active, \
    u.created_at, u.updated_at \
    FROM users u JOIN departments d ON d.id = u.department_id LEFT JOIN positions p ON p.id = u.position_id";

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: Uuid,
    pub full_name: String,
    pub login: String,
    pub email: String,
    pub password_hash: String,
    pub role: Role,
    pub department_id: Uuid,
    pub department_name: String,
    pub position_id: Option<Uuid>,
    pub position_name: Option<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Department {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Position {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct DocumentType {
    pub id: Uuid,
    pub name: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct Correspondent {
    pub id: Uuid,
    pub name: String,
    pub inn: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub signer_full_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const DOCUMENT_SELECT: &str = "SELECT d.id, d.registration_number, d.received_date, d.registration_date, \
    d.correspondent_id, c.name AS correspondent_name, c.inn AS correspondent_inn, d.addressee, d.summary, \
    d.document_type_id, t.name AS document_type_name, d.page_count, d.execution_deadline, d.status, \
    d.created_by, d.archived_at, d.created_at, d.updated_at \
    FROM incoming_documents d \
    JOIN correspondents c ON c.id = d.correspondent_id \
    JOIN document_types t ON t.id = d.document_type_id";

#[derive(Debug, Clone, FromRow)]
pub struct Document {
    pub id: Uuid,
    pub registration_number: String,
    pub received_date: NaiveDate,
    pub registration_date: NaiveDate,
    pub correspondent_id: Uuid,
    pub correspondent_name: String,
    pub correspondent_inn: Option<String>,
    pub addressee: Option<String>,
    pub summary: String,
    pub document_type_id: Uuid,
    pub document_type_name: String,
    pub page_count: i32,
    pub execution_deadline: NaiveDate,
    pub status: DocumentStatus,
    pub created_by: Uuid,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const RESOLUTION_SELECT: &str = "SELECT r.id, r.document_id, r.text, r.author_id, a.full_name AS author_name, \
    r.assigned_executor_id, e.full_name AS executor_name, r.deadline, r.status, r.executed_at, \
    r.created_at, r.updated_at \
    FROM resolutions r JOIN users a ON a.id = r.author_id JOIN users e ON e.id = r.assigned_executor_id";

#[derive(Debug, Clone, FromRow)]
pub struct Resolution {
    pub id: Uuid,
    pub document_id: Uuid,
    pub text: String,
    pub author_id: Uuid,
    pub author_name: String,
    pub assigned_executor_id: Uuid,
    pub executor_name: String,
    pub deadline: NaiveDate,
    pub status: ResolutionStatus,
    pub executed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const ATTACHMENT_SELECT: &str = "SELECT f.id, f.document_id, f.original_filename, f.storage_key, \
    f.content_type, f.size_bytes, f.attachment_type, f.uploaded_by, u.full_name AS uploader_name, f.created_at \
    FROM attachments f JOIN users u ON u.id = f.uploaded_by";

#[derive(Debug, Clone, FromRow)]
pub struct Attachment {
    pub id: Uuid,
    pub document_id: Uuid,
    pub original_filename: String,
    pub storage_key: String,
    pub content_type: String,
    pub size_bytes: i32,
    pub attachment_type: AttachmentType,
    pub uploaded_by: Uuid,
    pub uploader_name: String,
    pub created_at: DateTime<Utc>,
}

pub const HISTORY_SELECT: &str = "SELECT h.id, h.document_id, h.user_id, u.full_name AS user_name, h.action, \
    h.old_status, h.new_status, h.comment, h.metadata, h.created_at \
    FROM document_history h JOIN users u ON u.id = h.user_id";

#[derive(Debug, Clone, FromRow)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub document_id: Uuid,
    pub user_id: Uuid,
    pub user_name: String,
    pub action: String,
    pub old_status: Option<DocumentStatus>,
    pub new_status: Option<DocumentStatus>,
    pub comment: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}
