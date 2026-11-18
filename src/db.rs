//! Пул соединений PostgreSQL и миграции схемы (каталог `migrations/`, встраивается в бинарный файл).

use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Подключение с повторами: при старте контейнера БД может быть еще недоступна.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(err) if attempt < 10 => {
                tracing::warn!("БД недоступна (попытка {attempt}): {err}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
