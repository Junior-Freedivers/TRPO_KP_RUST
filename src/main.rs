//! Точка входа. Команды:
//!   correspondence [serve]   — миграции БД, подготовка бакета, запуск HTTP-сервера (по умолчанию)
//!   correspondence migrate   — только миграции
//!   correspondence seed      — миграции + демонстрационные данные
//!   correspondence healthcheck — проверка GET /health (для HEALTHCHECK в Docker)

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use correspondence::{Settings, build_router, db, init_state, seed};
use tracing_subscriber::EnvFilter;

fn configure_logging(level: &str) {
    // логирование в stdout (для Docker); пароли, JWT и ключи хранилища в логи не попадают
    let level = level.to_lowercase();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!("{level},aws_config=warn,aws_smithy_runtime=warn,aws_sdk_s3=warn,hyper=warn,sqlx=warn"))
    });
    tracing_subscriber::fmt().with_env_filter(filter).with_target(true).init();
}

fn load_settings() -> Result<Settings, ExitCode> {
    Settings::from_env().map_err(|err| {
        eprintln!("{err}");
        ExitCode::from(2)
    })
}

async fn serve() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    let addr = settings.bind_addr.clone();
    let state = init_state(settings).await?;
    // сервис стартует сразу; бакет создается, как только хранилище станет доступно (недоступность видна в /health)
    let storage = state.storage.clone();
    tokio::spawn(async move {
        for attempt in 1.. {
            match storage.ensure_bucket().await {
                Ok(()) => break,
                Err(err) => {
                    if attempt == 1 || attempt % 20 == 0 {
                        tracing::error!("Не удалось подготовить бакет файлового хранилища (попытка {attempt}): {err}");
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Сервис запущен: http://{addr} (UI: /ui, Swagger: /docs)");
    let pool = state.db.clone();
    axum::serve(listener, build_router(state)).with_graceful_shutdown(shutdown_signal()).await?;
    pool.close().await;
    tracing::info!("Сервис остановлен");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("Получен сигнал завершения, обрабатываю текущие запросы...");
}

/// Простой HTTP-запрос без внешних утилит (в образе нет curl/wget).
fn healthcheck() -> ExitCode {
    let port = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|a| a.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()))
        .unwrap_or(8000);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let result = (|| -> std::io::Result<String> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    })();
    match result {
        Ok(response) if response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200") => ExitCode::SUCCESS,
        Ok(response) => {
            eprintln!("{}", response.lines().next().unwrap_or("нет ответа"));
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("healthcheck: {err}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();
    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    if command == "healthcheck" {
        return healthcheck();
    }
    let settings = match load_settings() {
        Ok(s) => s,
        Err(code) => return code,
    };
    configure_logging(&settings.log_level);

    let result = match command.as_str() {
        "serve" => serve().await,
        "migrate" => async {
            let pool = db::connect(&settings.database_url).await?;
            db::migrate(&pool).await?;
            println!("Миграции применены");
            Ok(())
        }
        .await,
        "seed" => async {
            let state = init_state(settings).await?;
            seed::seed(&state.db, &state.settings).await
        }
        .await,
        other => {
            eprintln!("Неизвестная команда: {other}. Допустимы: serve, migrate, seed, healthcheck");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err:#}");
            eprintln!("Ошибка: {err:#}");
            ExitCode::FAILURE
        }
    }
}
