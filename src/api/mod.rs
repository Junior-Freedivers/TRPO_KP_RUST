//! HTTP-слой: обработчики REST API /api/v1, маршрутизация, OpenAPI (Swagger UI на /docs).
//!
//! Обработчики тонкие: объявляют путь, схемы, требуемые права и вызывают одну функцию сервиса.

mod auth;
mod correspondents;
mod dictionaries;
mod files;
mod health;
mod incoming;
mod reports;
mod users;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::Redirect;
use axum::routing::{get, patch, post};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

use crate::error::{method_not_allowed_fallback, not_found_fallback};
use crate::state::AppState;

pub use health::health;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Информационная система учета входящей корреспонденции",
        version = "2.0.0",
        description = "REST API: регистрация, поиск, резолюции, исполнение, контроль сроков, файлы, история, \
                       отчеты (PDF) и выгрузки (CSV). Авторизация: JWT (кнопка Authorize → токен из \
                       `POST /api/v1/auth/login`)."
    ),
    paths(
        auth::login, auth::me,
        users::create_user, users::list_users, users::list_executors, users::get_user, users::update_user,
        users::deactivate_user,
        dictionaries::list_departments, dictionaries::create_department, dictionaries::update_department,
        dictionaries::list_positions, dictionaries::create_position, dictionaries::update_position,
        dictionaries::list_document_types, dictionaries::create_document_type, dictionaries::update_document_type,
        correspondents::create_correspondent, correspondents::list_correspondents,
        correspondents::get_correspondent, correspondents::update_correspondent,
        incoming::register_document, incoming::list_documents, incoming::get_by_number, incoming::get_document,
        incoming::update_document, incoming::delete_document, incoming::get_history, incoming::change_status,
        incoming::create_resolution, incoming::list_resolutions, incoming::upload_file,
        incoming::update_resolution,
        files::download_file,
        reports::generate_report, reports::export_registration_log,
        health::health,
    ),
    components(schemas(crate::error::ErrorResponse, crate::dto::UploadForm)),
    modifiers(&BearerAuth),
    tags(
        (name = "auth", description = "Вход и текущий пользователь"),
        (name = "users", description = "Управление пользователями (admin)"),
        (name = "departments", description = "Подразделения (admin)"),
        (name = "positions", description = "Должности (admin)"),
        (name = "document-types", description = "Типы документов"),
        (name = "correspondents", description = "Корреспонденты (делопроизводитель)"),
        (name = "incoming", description = "Входящие документы, статусы, история, резолюции, файлы"),
        (name = "resolutions", description = "Редактирование резолюций (руководитель)"),
        (name = "files", description = "Скачивание вложений"),
        (name = "reports", description = "PDF-отчеты"),
        (name = "exports", description = "CSV-выгрузка журнала регистрации"),
        (name = "health", description = "Состояние сервиса"),
    )
)]
pub struct ApiDoc;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("JWT-токен, полученный в POST /api/v1/auth/login"))
                    .build(),
            ),
        );
    }
}

fn api_router(state: &AppState) -> Router<AppState> {
    // лимит тела для загрузки файлов: MAX_UPLOAD_SIZE_MB + запас на служебные поля multipart
    let upload_limit = state.settings.max_upload_bytes() + 1024 * 1024;
    Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/me", get(auth::me))
        .route("/users", post(users::create_user).get(users::list_users))
        .route("/users/executors", get(users::list_executors))
        .route("/users/{user_id}", get(users::get_user).patch(users::update_user))
        .route("/users/{user_id}/deactivate", post(users::deactivate_user))
        .route("/departments", get(dictionaries::list_departments).post(dictionaries::create_department))
        .route("/departments/{item_id}", patch(dictionaries::update_department))
        .route("/positions", get(dictionaries::list_positions).post(dictionaries::create_position))
        .route("/positions/{item_id}", patch(dictionaries::update_position))
        .route("/document-types", get(dictionaries::list_document_types).post(dictionaries::create_document_type))
        .route("/document-types/{item_id}", patch(dictionaries::update_document_type))
        .route("/correspondents", post(correspondents::create_correspondent).get(correspondents::list_correspondents))
        .route(
            "/correspondents/{correspondent_id}",
            get(correspondents::get_correspondent).patch(correspondents::update_correspondent),
        )
        .route("/incoming", post(incoming::register_document).get(incoming::list_documents))
        .route("/incoming/by-registration-number/{*registration_number}", get(incoming::get_by_number))
        .route(
            "/incoming/{document_id}",
            get(incoming::get_document).patch(incoming::update_document).delete(incoming::delete_document),
        )
        .route("/incoming/{document_id}/history", get(incoming::get_history))
        .route("/incoming/{document_id}/status", post(incoming::change_status))
        .route(
            "/incoming/{document_id}/resolutions",
            post(incoming::create_resolution).get(incoming::list_resolutions),
        )
        .route(
            "/incoming/{document_id}/files",
            post(incoming::upload_file).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route("/resolutions/{resolution_id}", patch(incoming::update_resolution))
        .route("/files/{attachment_id}/download", get(files::download_file))
        .route("/reports/generate", post(reports::generate_report))
        .route("/exports/registration-log", get(reports::export_registration_log))
        .route("/health", get(health::health))
}

pub fn build_router(state: AppState) -> Router {
    let ui_dir = state.settings.ui_dir.clone();
    Router::new()
        .nest("/api/v1", api_router(&state))
        // /health — для Docker и балансировщиков
        .route("/health", get(health::health))
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        // веб-интерфейс: статические файлы; все данные он получает через REST API /api/v1
        .nest_service("/ui", ServeDir::new(ui_dir).append_index_html_on_directories(true))
        .merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
        .fallback(not_found_fallback)
        .method_not_allowed_fallback(method_not_allowed_fallback)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
