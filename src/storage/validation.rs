//! Проверка загружаемых файлов: размер, расширение, MIME-тип и сигнатура содержимого.

use crate::error::{AppError, AppResult};

pub const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub const ALLOWED_HUMAN: &str = "PDF, JPG, PNG, DOCX";

struct AllowedType {
    extension: &'static str,
    mimes: &'static [&'static str],
    canonical: &'static str,
    signature: &'static [u8],
}

const JPEG_MIMES: &[&str] = &["image/jpeg", "image/jpg", "image/pjpeg"];

const ALLOWED_TYPES: &[AllowedType] = &[
    AllowedType { extension: ".pdf", mimes: &["application/pdf"], canonical: "application/pdf", signature: b"%PDF" },
    AllowedType { extension: ".jpg", mimes: JPEG_MIMES, canonical: "image/jpeg", signature: b"\xff\xd8\xff" },
    AllowedType { extension: ".jpeg", mimes: JPEG_MIMES, canonical: "image/jpeg", signature: b"\xff\xd8\xff" },
    AllowedType { extension: ".png", mimes: &["image/png"], canonical: "image/png", signature: b"\x89PNG\r\n\x1a\n" },
    AllowedType { extension: ".docx", mimes: &[DOCX_MIME], canonical: DOCX_MIME, signature: b"PK\x03\x04" },
];

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedFile {
    /// безопасное исходное имя (только для отображения, не для путей)
    pub filename: String,
    /// нормализованное расширение для storage_key
    pub extension: String,
    /// канонический MIME
    pub content_type: String,
    pub size: usize,
}

/// Оставляет только базовое имя: отбрасывает каталоги (в т.ч. «..\..\») и управляющие символы.
pub fn sanitize_filename(name: Option<&str>) -> String {
    let base = name.unwrap_or("").rsplit(['\\', '/']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !matches!(*c as u32, 0x00..=0x1f | 0x7f)).collect();
    let trimmed = cleaned.trim().trim_matches('.');
    trimmed.chars().take(255).collect()
}

/// `too_large` — признак того, что тело запроса превысило лимит еще при чтении.
pub fn validate_upload(
    filename: Option<&str>,
    content_type: Option<&str>,
    data: &[u8],
    too_large: bool,
    max_bytes: usize,
) -> AppResult<ValidatedFile> {
    let safe_name = sanitize_filename(filename);
    if safe_name.is_empty() {
        return Err(AppError::validation("Не указано имя файла").with_code("INVALID_FILENAME").with_field("file"));
    }
    if too_large || data.len() > max_bytes {
        return Err(AppError::too_large(format!("Размер файла превышает {} МБ", max_bytes / (1024 * 1024)))
            .with_field("file"));
    }
    if data.is_empty() {
        return Err(AppError::validation("Файл пустой").with_code("EMPTY_FILE").with_field("file"));
    }

    let extension = match safe_name.rsplit_once('.') {
        Some((_, ext)) => format!(".{}", ext.to_lowercase()),
        None => String::new(),
    };
    let unsupported = |what: &str| {
        AppError::validation(format!("Недопустимое {what} файла. Разрешены: {ALLOWED_HUMAN}"))
            .with_code("UNSUPPORTED_FILE_TYPE")
            .with_field("file")
    };
    let Some(allowed) = ALLOWED_TYPES.iter().find(|t| t.extension == extension) else {
        return Err(unsupported("расширение"));
    };
    let mime = content_type.unwrap_or("").split(';').next().unwrap_or("").trim().to_lowercase();
    if !allowed.mimes.contains(&mime.as_str()) {
        return Err(AppError::validation(format!("Недопустимый MIME-тип файла. Разрешены: {ALLOWED_HUMAN}"))
            .with_code("UNSUPPORTED_FILE_TYPE")
            .with_field("file"));
    }
    if !data.starts_with(allowed.signature) {
        return Err(AppError::validation("Содержимое файла не соответствует его расширению")
            .with_code("FILE_CONTENT_MISMATCH")
            .with_field("file"));
    }
    Ok(ValidatedFile {
        filename: safe_name,
        extension: if extension == ".jpeg" { ".jpg".into() } else { extension },
        content_type: allowed.canonical.to_string(),
        size: data.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n%%EOF";
    const MAX: usize = 1024;

    #[test]
    fn sanitize_strips_directories_and_control_chars() {
        assert_eq!(sanitize_filename(Some("..\\..\\etc/passwd")), "passwd");
        assert_eq!(sanitize_filename(Some("../../скан.pdf")), "скан.pdf");
        assert_eq!(sanitize_filename(Some("a\u{0}b\nc.pdf")), "abc.pdf");
        assert_eq!(sanitize_filename(Some("...")), "");
        assert_eq!(sanitize_filename(None), "");
    }

    #[test]
    fn valid_pdf() {
        let file = validate_upload(Some("Скан.PDF"), Some("application/pdf"), PDF, false, MAX).unwrap();
        assert_eq!(file.extension, ".pdf");
        assert_eq!(file.content_type, "application/pdf");
        assert_eq!(file.filename, "Скан.PDF");
    }

    #[test]
    fn jpeg_is_normalized() {
        let file = validate_upload(Some("a.jpeg"), Some("image/jpg"), b"\xff\xd8\xff\xe0", false, MAX).unwrap();
        assert_eq!(file.extension, ".jpg");
        assert_eq!(file.content_type, "image/jpeg");
    }

    #[test]
    fn rejections() {
        let code = |r: AppResult<ValidatedFile>| r.unwrap_err().code;
        assert_eq!(code(validate_upload(Some("a.exe"), Some("application/pdf"), PDF, false, MAX)), "UNSUPPORTED_FILE_TYPE");
        assert_eq!(code(validate_upload(Some("a.pdf"), Some("text/plain"), PDF, false, MAX)), "UNSUPPORTED_FILE_TYPE");
        assert_eq!(code(validate_upload(Some("a.png"), Some("image/png"), PDF, false, MAX)), "FILE_CONTENT_MISMATCH");
        assert_eq!(code(validate_upload(Some("a.pdf"), Some("application/pdf"), b"", false, MAX)), "EMPTY_FILE");
        assert_eq!(code(validate_upload(Some(""), Some("application/pdf"), PDF, false, MAX)), "INVALID_FILENAME");
        assert_eq!(code(validate_upload(Some("a.pdf"), Some("application/pdf"), &[b'%'; 2048], false, MAX)), "FILE_TOO_LARGE");
        assert_eq!(code(validate_upload(Some("a.pdf"), Some("application/pdf"), PDF, true, MAX)), "FILE_TOO_LARGE");
    }
}
