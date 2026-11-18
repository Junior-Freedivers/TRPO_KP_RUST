//! Прикладные ошибки и единый формат ответа API: `{"detail": ..., "code": ..., "field": ...}`.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// Тело ответа об ошибке.
#[derive(Debug, Serialize, ToSchema)]
#[schema(example = json!({"detail": "Регистрационный номер уже существует",
    "code": "REGISTRATION_NUMBER_CONFLICT", "field": "registration_number"}))]
pub struct ErrorResponse {
    pub detail: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub status: StatusCode,
    pub code: String,
    pub detail: String,
    pub field: Option<String>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(status: StatusCode, code: &str, detail: impl Into<String>) -> Self {
        Self { status, code: code.to_string(), detail: detail.into(), field: None }
    }

    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", detail)
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "FORBIDDEN", detail)
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", detail)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "CONFLICT", detail)
    }

    pub fn validation(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR", detail)
    }

    pub fn too_large(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE", detail)
    }

    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", "Внутренняя ошибка сервера")
    }

    /// Ошибка валидации конкретного поля (detail в формате «поле: сообщение»).
    pub fn invalid_field(field: &str, message: impl AsRef<str>) -> Self {
        Self::validation(format!("{field}: {}", message.as_ref())).with_field(field)
    }

    pub fn with_code(mut self, code: &str) -> Self {
        self.code = code.to_string();
        self
    }

    pub fn with_field(mut self, field: &str) -> Self {
        self.field = Some(field.to_string());
        self
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.code, self.detail)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let unauthorized = self.status == StatusCode::UNAUTHORIZED;
        let body = ErrorResponse { detail: self.detail, code: self.code, field: self.field };
        let mut response = (self.status, Json(body)).into_response();
        if unauthorized {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &err
            && db.code().is_some_and(|c| c.starts_with("23"))
        {
            // 23xxx — нарушение ограничений целостности (unique, FK, check, not null)
            tracing::warn!("Нарушение ограничения целостности БД: {}", db.message());
            return AppError::conflict("Операция нарушает ограничения целостности данных");
        }
        tracing::error!("Ошибка БД: {err}");
        AppError::internal()
    }
}

impl From<crate::storage::StorageError> for AppError {
    fn from(err: crate::storage::StorageError) -> Self {
        tracing::error!("Ошибка файлового хранилища: {err}");
        AppError::internal()
    }
}

/// Обработчик неизвестных маршрутов: тот же формат ошибки, что и у остального API.
pub async fn not_found_fallback() -> AppError {
    AppError::not_found("Not Found")
}

pub async fn method_not_allowed_fallback() -> AppError {
    AppError::new(StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED", "Method Not Allowed")
}
