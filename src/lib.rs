//! Информационная система учета входящей корреспонденции организации — backend на Rust.
//!
//! Стек: axum (HTTP), sqlx (PostgreSQL), aws-sdk-s3 (файловое хранилище SeaweedFS), genpdf (PDF-отчеты).

pub mod api;
pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod domain;
pub mod dto;
pub mod error;
pub mod extract;
pub mod models;
pub mod reports;
pub mod security;
pub mod seed;
pub mod services;
pub mod state;
pub mod storage;

use std::sync::Arc;

pub use api::build_router;
pub use config::Settings;
pub use state::AppState;

/// Подключение к БД, миграции и клиент хранилища — общая подготовка для сервера, seed и тестов.
pub async fn init_state(settings: Settings) -> anyhow::Result<AppState> {
    let db = db::connect(&settings.database_url).await?;
    db::migrate(&db).await?;
    let storage = storage::Storage::new(&settings);
    Ok(AppState { db, settings: Arc::new(settings), storage })
}
