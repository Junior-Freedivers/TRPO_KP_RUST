//! Экстракторы запросов с ошибками в едином формате API (422 VALIDATION_ERROR, а не текст axum).

use std::sync::LazyLock;

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::header;
use axum::http::request::Parts;
use bytes::Bytes;
use regex::Regex;
use serde::de::DeserializeOwned;

use crate::error::AppError;

static POSITION_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" at line \d+ column \d+$").unwrap());
static BACKTICK_FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());

/// Преобразует ошибку serde с путем к полю в ответ 422 `{detail: "поле: сообщение", field}`.
fn deserialize_error(path: String, message: String) -> AppError {
    let message = POSITION_SUFFIX.replace(&message, "").to_string();
    let mut field = (path != "." && !path.is_empty()).then_some(path);
    if message.starts_with("missing field") {
        let missing = BACKTICK_FIELD.captures(&message).map(|c| c[1].to_string());
        if let Some(name) = missing {
            field = Some(match field {
                Some(parent) => format!("{parent}.{name}"),
                None => name,
            });
            let field = field.unwrap();
            let leaf = field.rsplit('.').next().unwrap_or(&field).to_string();
            return AppError::validation(format!("{field}: Обязательное поле")).with_field(&leaf);
        }
    }
    match field {
        Some(f) => {
            let leaf = f.rsplit('.').next().unwrap_or(&f).to_string();
            AppError::validation(format!("{f}: {message}")).with_field(&leaf)
        }
        None => AppError::validation(message),
    }
}

/// JSON-тело запроса.
pub struct Json<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Json<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"));
        let body = Bytes::from_request(req, state)
            .await
            .map_err(|e| AppError::validation(format!("Не удалось прочитать тело запроса: {e}")))?;
        if !is_json {
            return Err(AppError::validation("Ожидается тело запроса в формате JSON (Content-Type: application/json)"));
        }
        let mut deserializer = serde_json::Deserializer::from_slice(&body);
        match serde_path_to_error::deserialize(&mut deserializer) {
            Ok(value) => Ok(Json(value)),
            Err(err) => {
                let path = err.path().to_string();
                let inner = err.into_inner();
                if inner.is_syntax() || inner.is_eof() {
                    return Err(AppError::validation(format!("Некорректный JSON: {inner}")));
                }
                Err(deserialize_error(path, inner.to_string()))
            }
        }
    }
}

/// Параметры строки запроса.
pub struct Query<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Query<T> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let query = parts.uri.query().unwrap_or("");
        let deserializer = serde_urlencoded::Deserializer::new(form_urlencoded::parse(query.as_bytes()));
        serde_path_to_error::deserialize(deserializer).map(Query).map_err(|err| {
            let path = err.path().to_string();
            deserialize_error(path, err.into_inner().to_string())
        })
    }
}

/// Параметр пути (UUID и т. п.): ошибка разбора — 422, как в прежней версии API.
pub struct Path<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Path<T> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Path(value)),
            Err(rejection) => Err(AppError::validation(format!("Некорректный параметр пути: {}", rejection.body_text()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_field_message() {
        let err = deserialize_error(".".into(), "missing field `summary` at line 1 column 2".into());
        assert_eq!(err.detail, "summary: Обязательное поле");
        assert_eq!(err.field.as_deref(), Some("summary"));
    }

    #[test]
    fn invalid_value_message() {
        let err = deserialize_error("status".into(), "unknown variant `x`, expected one of ...".into());
        assert_eq!(err.field.as_deref(), Some("status"));
        assert!(err.detail.starts_with("status: unknown variant"));
    }
}
