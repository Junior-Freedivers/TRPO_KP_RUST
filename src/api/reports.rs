use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;

use crate::auth::{Authorized, can};
use crate::dto::{RegistrationLogQuery, ReportRequest};
use crate::error::{AppResult, ErrorResponse};
use crate::extract::{Json, Query};
use crate::services::{exports, reports};
use crate::state::AppState;

/// Сформировать PDF-отчет (clerk, manager)
///
/// Типы: `documents` (по документам), `executors` (по исполнителям), `deadlines` (по срокам исполнения).
/// Период отбирается по дате регистрации документов.
#[utoipa::path(post, path = "/api/v1/reports/generate", tag = "reports", security(("bearer" = [])),
    request_body = ReportRequest,
    responses(
        (status = 200, description = "PDF-файл отчета", content_type = "application/pdf"),
        (status = 403, body = ErrorResponse), (status = 422, body = ErrorResponse),
    ))]
pub async fn generate_report(
    State(state): State<AppState>,
    auth: Authorized<can::GenerateReport>,
    Json(data): Json<ReportRequest>,
) -> AppResult<impl IntoResponse> {
    let (pdf, filename) = reports::generate(&state, &auth.user, data).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        pdf,
    ))
}

/// Выгрузка журнала регистрации в CSV (clerk)
#[utoipa::path(get, path = "/api/v1/exports/registration-log", tag = "exports", security(("bearer" = [])),
    params(RegistrationLogQuery),
    responses(
        (status = 200, description = "CSV: UTF-8 с BOM, разделитель «;»", content_type = "text/csv"),
        (status = 403, body = ErrorResponse), (status = 422, body = ErrorResponse),
    ))]
pub async fn export_registration_log(
    State(state): State<AppState>,
    auth: Authorized<can::ExportRegistry>,
    Query(query): Query<RegistrationLogQuery>,
) -> AppResult<impl IntoResponse> {
    let csv = exports::registration_log(&state, &auth.user, query).await?;
    let filename = format!("registration-log-{}.csv", state.settings.business_today().format("%Y-%m-%d"));
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        csv,
    ))
}
