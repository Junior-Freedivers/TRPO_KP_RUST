//! Настройки приложения. Все секреты — только из окружения / .env, в коде их нет.

use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::domain::reg_number;

#[derive(Debug, Clone)]
pub struct Settings {
    pub database_url: String,
    pub jwt_secret: String,
    pub access_token_expire_minutes: i64,

    pub s3_endpoint: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_bucket: String,
    pub s3_region: String,

    pub max_upload_size_mb: u64,
    pub reg_number_template: String,
    pub deadline_warning_days: i64,

    /// Часовой пояс организации: по нему определяются дата регистрации, «сегодня» и просрочка.
    pub timezone: Tz,
    pub log_level: String,
    /// Путь к TTF с кириллицей (по умолчанию ищется DejaVu Sans).
    pub pdf_font_path: Option<String>,

    pub bind_addr: String,
    pub ui_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("Ошибка конфигурации: {0}")]
pub struct ConfigError(pub String);

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn required(name: &str) -> Result<String, ConfigError> {
    var(name).ok_or_else(|| ConfigError(format!("не задана переменная окружения {name}")))
}

fn parse_num<T: std::str::FromStr>(name: &str, default: T) -> Result<T, ConfigError> {
    match var(name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| ConfigError(format!("{name}: ожидается число, получено «{v}»"))),
    }
}

/// Приводит URL вида `postgresql+asyncpg://...` (формат прежней Python-версии) к `postgres://...`.
pub fn normalize_database_url(url: &str) -> String {
    for prefix in ["postgresql+asyncpg://", "postgresql+psycopg://", "postgresql://"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return format!("postgres://{rest}");
        }
    }
    url.to_string()
}

impl Settings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let jwt_secret = required("JWT_SECRET")?;
        if jwt_secret.chars().count() < 16 {
            return Err(ConfigError("JWT_SECRET должен содержать не менее 16 символов".into()));
        }
        let access_token_expire_minutes: i64 = parse_num("ACCESS_TOKEN_EXPIRE_MINUTES", 60)?;
        if access_token_expire_minutes < 1 {
            return Err(ConfigError("ACCESS_TOKEN_EXPIRE_MINUTES должен быть >= 1".into()));
        }
        let max_upload_size_mb: u64 = parse_num("MAX_UPLOAD_SIZE_MB", 20)?;
        if max_upload_size_mb < 1 {
            return Err(ConfigError("MAX_UPLOAD_SIZE_MB должен быть >= 1".into()));
        }
        let deadline_warning_days: i64 = parse_num("DEADLINE_WARNING_DAYS", 3)?;
        if deadline_warning_days < 0 {
            return Err(ConfigError("DEADLINE_WARNING_DAYS должен быть >= 0".into()));
        }
        let reg_number_template = var("REG_NUMBER_TEMPLATE").unwrap_or_else(|| "ВХ-{YYYY}-{NNNNNN}".into());
        reg_number::validate_template(&reg_number_template).map_err(ConfigError)?;

        let app_timezone = var("APP_TIMEZONE").unwrap_or_else(|| "Europe/Moscow".into());
        let timezone: Tz = app_timezone
            .parse()
            .map_err(|_| ConfigError(format!("Неизвестный часовой пояс APP_TIMEZONE: {app_timezone}")))?;

        Ok(Self {
            database_url: normalize_database_url(&required("DATABASE_URL")?),
            jwt_secret,
            access_token_expire_minutes,
            s3_endpoint: required("S3_ENDPOINT")?,
            s3_access_key: required("S3_ACCESS_KEY")?,
            s3_secret_key: required("S3_SECRET_KEY")?,
            s3_bucket: required("S3_BUCKET")?,
            s3_region: var("S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            max_upload_size_mb,
            reg_number_template,
            deadline_warning_days,
            timezone,
            log_level: var("LOG_LEVEL").unwrap_or_else(|| "INFO".into()),
            pdf_font_path: var("PDF_FONT_PATH"),
            bind_addr: var("BIND_ADDR").unwrap_or_else(|| "0.0.0.0:8000".into()),
            ui_dir: PathBuf::from(var("UI_DIR").unwrap_or_else(|| "ui".into())),
        })
    }

    pub fn max_upload_bytes(&self) -> usize {
        (self.max_upload_size_mb * 1024 * 1024) as usize
    }

    /// Текущий момент в часовом поясе организации (APP_TIMEZONE).
    pub fn business_now(&self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.timezone)
    }

    /// «Сегодня» в часовом поясе организации: дата регистрации и контроль сроков.
    pub fn business_today(&self) -> NaiveDate {
        self.business_now().date_naive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_url_is_normalized() {
        assert_eq!(normalize_database_url("postgresql+asyncpg://u:p@h:5432/db"), "postgres://u:p@h:5432/db");
        assert_eq!(normalize_database_url("postgres://u:p@h/db"), "postgres://u:p@h/db");
    }
}
