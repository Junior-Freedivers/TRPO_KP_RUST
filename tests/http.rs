//! HTTP-слой без внешней инфраструктуры: маршрутизация, веб-интерфейс, OpenAPI, ошибки авторизации, health.
//! БД и хранилище указывают на недоступные адреса — проверяется поведение сервиса без зависимостей.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, header};
use common::send;
use correspondence::storage::Storage;
use correspondence::{AppState, Settings, build_router};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;

fn offline_app() -> axum::Router {
    let settings = Settings {
        database_url: "postgres://user:pass@127.0.0.1:1/none".into(),
        jwt_secret: "test-secret-test-secret-test-secret-123".into(),
        access_token_expire_minutes: 60,
        s3_endpoint: "http://127.0.0.1:1".into(),
        s3_access_key: "key".into(),
        s3_secret_key: "secret".into(),
        s3_bucket: "correspondence-test".into(),
        s3_region: "us-east-1".into(),
        max_upload_size_mb: 1,
        reg_number_template: "ВХ-{YYYY}-{NNNNNN}".into(),
        deadline_warning_days: 3,
        timezone: chrono_tz::Europe::Moscow,
        log_level: "INFO".into(),
        pdf_font_path: None,
        bind_addr: "127.0.0.1:0".into(),
        ui_dir: "ui".into(),
    };
    let db = PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(1))
        .connect_lazy(&settings.database_url)
        .unwrap();
    let storage = Storage::new(&settings);
    build_router(AppState { db, settings: Arc::new(settings), storage })
}

async fn get(app: &axum::Router, uri: &str) -> common::Resp {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn ui_is_served_and_root_redirects() {
    let app = offline_app();
    let root = get(&app, "/").await;
    assert!(matches!(root.status.as_u16(), 302 | 307));
    assert_eq!(root.header("location"), "/ui/");

    let page = get(&app, "/ui/").await;
    assert_eq!(page.status.as_u16(), 200);
    assert!(page.header("content-type").contains("text/html"));
    assert!(page.text().contains("/ui/app.js") && page.text().contains("/ui/styles.css"));

    for (asset, mime) in [("app.js", "javascript"), ("api.js", "javascript"), ("styles.css", "text/css")] {
        let response = get(&app, &format!("/ui/{asset}")).await;
        assert_eq!(response.status.as_u16(), 200, "{asset}");
        assert!(response.header("content-type").contains(mime), "{asset}: {}", response.header("content-type"));
    }
}

const REQUIRED: &[(&str, &str)] = &[
    ("post", "/api/v1/auth/login"), ("get", "/api/v1/auth/me"),
    ("post", "/api/v1/users"), ("get", "/api/v1/users"), ("get", "/api/v1/users/{user_id}"),
    ("patch", "/api/v1/users/{user_id}"), ("post", "/api/v1/users/{user_id}/deactivate"),
    ("get", "/api/v1/departments"), ("post", "/api/v1/departments"), ("patch", "/api/v1/departments/{item_id}"),
    ("get", "/api/v1/positions"), ("post", "/api/v1/positions"), ("patch", "/api/v1/positions/{item_id}"),
    ("get", "/api/v1/document-types"), ("post", "/api/v1/document-types"),
    ("patch", "/api/v1/document-types/{item_id}"),
    ("post", "/api/v1/correspondents"), ("get", "/api/v1/correspondents"),
    ("get", "/api/v1/correspondents/{correspondent_id}"), ("patch", "/api/v1/correspondents/{correspondent_id}"),
    ("post", "/api/v1/incoming"), ("get", "/api/v1/incoming"), ("get", "/api/v1/incoming/{document_id}"),
    ("get", "/api/v1/incoming/by-registration-number/{registration_number}"),
    ("patch", "/api/v1/incoming/{document_id}"), ("delete", "/api/v1/incoming/{document_id}"),
    ("post", "/api/v1/incoming/{document_id}/resolutions"), ("get", "/api/v1/incoming/{document_id}/resolutions"),
    ("patch", "/api/v1/resolutions/{resolution_id}"),
    ("post", "/api/v1/incoming/{document_id}/files"), ("get", "/api/v1/files/{attachment_id}/download"),
    ("get", "/api/v1/incoming/{document_id}/history"), ("post", "/api/v1/incoming/{document_id}/status"),
    ("post", "/api/v1/reports/generate"), ("get", "/api/v1/exports/registration-log"), ("get", "/health"),
];

#[tokio::test]
async fn swagger_and_all_required_endpoints_are_published() {
    let app = offline_app();
    let docs = get(&app, "/docs/").await;
    assert_eq!(docs.status.as_u16(), 200);
    assert!(docs.header("content-type").contains("text/html"));
    let bare = get(&app, "/docs").await; // ссылка из README: браузер попадает на Swagger UI
    assert!(bare.status.is_success() || bare.header("location").ends_with("/docs/"), "{} {}", bare.status, bare.header("location"));

    let schema = get(&app, "/openapi.json").await.json();
    for (method, path) in REQUIRED {
        assert!(schema["paths"][path][method].is_object(), "нет {method} {path} в OpenAPI");
    }
    assert!(!schema["components"]["schemas"].to_string().contains("password_hash"));
    assert!(schema["components"]["securitySchemes"]["bearer"].is_object());
}

#[tokio::test]
async fn missing_or_invalid_token_returns_401_without_touching_db() {
    let app = offline_app();
    let missing = get(&app, "/api/v1/incoming").await;
    missing.assert_error(401, Some("UNAUTHORIZED"), None);
    assert_eq!(missing.header("www-authenticate"), "Bearer");

    let bad = send(
        &app,
        Request::get("/api/v1/incoming").header(header::AUTHORIZATION, "Bearer not-a-token").body(Body::empty()).unwrap(),
    )
    .await;
    bad.assert_error(401, Some("INVALID_TOKEN"), None);

    let basic = send(
        &app,
        Request::get("/api/v1/auth/me").header(header::AUTHORIZATION, "Basic YWRtaW46YWRtaW4=").body(Body::empty()).unwrap(),
    )
    .await;
    basic.assert_error(401, Some("UNAUTHORIZED"), None);
}

#[tokio::test]
async fn unsupported_methods_and_unknown_routes_use_error_format() {
    let app = offline_app();
    let history = "/api/v1/incoming/00000000-0000-0000-0000-000000000001/history";
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let response = send(&app, Request::builder().method(method).uri(history).body(Body::empty()).unwrap()).await;
        response.assert_error(405, Some("METHOD_NOT_ALLOWED"), None);
    }
    let delete_user = send(
        &app,
        Request::delete("/api/v1/users/00000000-0000-0000-0000-000000000001").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(delete_user.status.as_u16(), 405);
    get(&app, "/api/v1/unknown").await.assert_error(404, Some("NOT_FOUND"), None);
}

#[tokio::test]
async fn login_validation_happens_before_db_access() {
    let app = offline_app();
    let request = |body: Value| {
        Request::post("/api/v1/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    send(&app, request(json!({"login": "clerk"}))).await.assert_error(422, Some("VALIDATION_ERROR"), Some("password"));
    send(&app, request(json!({"login": "", "password": "x"}))).await.assert_error(422, Some("VALIDATION_ERROR"), Some("login"));
    let not_json = send(&app, Request::post("/api/v1/auth/login").body(Body::from("login=clerk")).unwrap()).await;
    not_json.assert_error(422, Some("VALIDATION_ERROR"), None);
}

#[tokio::test]
async fn health_reports_unavailable_dependencies_with_503() {
    let app = offline_app();
    for url in ["/health", "/api/v1/health"] {
        let response = get(&app, url).await;
        assert_eq!(response.status.as_u16(), 503, "{}", response.text());
        assert_eq!(response.json(), json!({"status": "degraded", "database": "unavailable", "storage": "unavailable"}));
    }
}
