use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::auth::{Authorized, can};
use crate::error::{AppError, AppResult, ErrorResponse};
use crate::extract::Path;
use crate::services::attachments;
use crate::state::AppState;

/// Символы, которые не кодируются (как `urllib.parse.quote` в прежней версии).
const FILENAME_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~').remove(b'/');

/// `Content-Disposition` с ASCII-именем и полным именем в UTF-8 (RFC 5987).
pub fn content_disposition(filename: &str) -> String {
    let ascii: String = filename.chars().filter(|c| c.is_ascii() && !matches!(c, '"' | '\\')).collect();
    let ascii = ascii.trim();
    let ascii = if ascii.is_empty() { "file" } else { ascii };
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", utf8_percent_encode(filename, FILENAME_SET))
}

/// Скачивание файла (после проверки прав доступа)
#[utoipa::path(get, path = "/api/v1/files/{attachment_id}/download", tag = "files", security(("bearer" = [])),
    params(("attachment_id" = Uuid, Path)),
    responses(
        (status = 200, description = "Содержимое файла", content_type = "application/octet-stream"),
        (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse),
    ))]
pub async fn download_file(
    State(state): State<AppState>,
    auth: Authorized<can::ViewDocuments>,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let attachment = attachments::get_for_download(&state, &auth.user, id).await?;
    let stream = state.storage.stream(&attachment.storage_key).await?;
    let body = Body::from_stream(ReaderStream::new(stream.into_async_read()));
    let mut response = body.into_response();
    let headers = response.headers_mut();
    let header_value = |v: String| HeaderValue::from_str(&v).map_err(|_| AppError::internal());
    headers.insert(header::CONTENT_TYPE, header_value(attachment.content_type.clone())?);
    headers.insert(header::CONTENT_LENGTH, header_value(attachment.size_bytes.to_string())?);
    headers.insert(header::CONTENT_DISPOSITION, header_value(content_disposition(&attachment.original_filename))?);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::content_disposition;

    #[test]
    fn cyrillic_filename() {
        assert_eq!(
            content_disposition("Скан \"1\".pdf"),
            "attachment; filename=\"1.pdf\"; filename*=UTF-8''%D0%A1%D0%BA%D0%B0%D0%BD%20%221%22.pdf"
        );
        assert_eq!(content_disposition("письмо"), "attachment; filename=\"file\"; filename*=UTF-8''%D0%BF%D0%B8%D1%81%D1%8C%D0%BC%D0%BE");
    }
}
