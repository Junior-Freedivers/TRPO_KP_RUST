//! Формирование PDF-отчетов: данные берутся из БД, расчеты — в domain::stats.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::domain::deadlines::deadline_state;
use crate::domain::stats::{DeadlineCategory, ResolutionFact, classify_deadline, compute_executor_stats};
use crate::dto::{DocumentFilters, ReportRequest, UserBrief};
use crate::error::{AppError, AppResult};
use crate::models::{DeadlineState, Document, DocumentStatus, ReportType, Role, User};
use crate::reports::pdf::{ReportData, render_report};
use crate::services::documents::search_all;
use crate::state::AppState;

fn role_label(role: Role) -> &'static str {
    match role {
        Role::Clerk => "делопроизводитель",
        Role::Manager => "руководитель",
        Role::Executor => "исполнитель",
        Role::Admin => "администратор",
    }
}

fn title(report_type: ReportType) -> &'static str {
    match report_type {
        ReportType::Documents => "Отчет по входящим документам",
        ReportType::Executors => "Отчет по исполнителям",
        ReportType::Deadlines => "Отчет по срокам исполнения",
    }
}

pub fn fmt_date(value: NaiveDate) -> String {
    value.format("%d.%m.%Y").to_string()
}

pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shorten(text: &str, limit: usize) -> String {
    let text = collapse_whitespace(text);
    if text.chars().count() <= limit {
        text
    } else {
        let mut cut: String = text.chars().take(limit - 1).collect();
        cut.push('…');
        cut
    }
}

/// ФИО исполнителей документа, по алфавиту, без повторов.
pub fn executor_names(executors: Option<&Vec<UserBrief>>, empty: &str) -> String {
    let mut names: Vec<&str> = executors.map(|list| list.iter().map(|u| u.full_name.as_str()).collect()).unwrap_or_default();
    names.sort();
    names.dedup();
    if names.is_empty() { empty.to_string() } else { names.join(", ") }
}

async fn filter_labels(state: &AppState, req: &ReportRequest) -> AppResult<Vec<String>> {
    let mut labels = Vec::new();
    if let Some(id) = req.correspondent_id {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM correspondents WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
        labels.push(format!("корреспондент: {}", name.unwrap_or_else(|| id.to_string())));
    }
    if let Some(id) = req.executor_id {
        let name: Option<String> =
            sqlx::query_scalar("SELECT full_name FROM users WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
        labels.push(format!("исполнитель: {}", name.unwrap_or_else(|| id.to_string())));
    }
    if let Some(status) = req.status {
        labels.push(format!("статус: {status}"));
    }
    if let Some(id) = req.document_type_id {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM document_types WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
        labels.push(format!("тип документа: {}", name.unwrap_or_else(|| id.to_string())));
    }
    Ok(labels)
}

/// Типы: `documents` (по документам), `executors` (по исполнителям), `deadlines` (по срокам исполнения).
/// Период отбирается по дате регистрации документов. Возвращает PDF и имя файла.
pub async fn generate(state: &AppState, actor: &User, req: ReportRequest) -> AppResult<(Vec<u8>, String)> {
    req.validate()?;
    let filters = DocumentFilters {
        date_from: Some(req.period_from),
        date_to: Some(req.period_to),
        correspondent_id: req.correspondent_id,
        executor_id: req.executor_id,
        status: req.status,
        document_type_id: req.document_type_id,
        ..Default::default()
    };
    let mut data = ReportData {
        title: title(req.report_type).into(),
        period: format!("{} — {} (по дате регистрации)", fmt_date(req.period_from), fmt_date(req.period_to)),
        generated_at: state.settings.business_now(),
        generated_by: format!("{} ({})", actor.full_name, role_label(actor.role)),
        columns: vec![],
        col_weights: vec![],
        rows: vec![],
        filters: filter_labels(state, &req).await?,
        summary: vec![],
    };

    match req.report_type {
        ReportType::Documents => {
            let (docs, executors) = search_all(state, &filters).await?;
            fill_documents(state, &mut data, &docs, &executors);
        }
        ReportType::Executors => fill_executors(state, &mut data, &req).await?,
        ReportType::Deadlines => {
            let (docs, _) = search_all(state, &filters).await?;
            fill_deadlines(state, &mut data, &docs).await?;
        }
    }

    let font = state.settings.pdf_font_path.clone();
    let rows = data.rows.len();
    let pdf = tokio::task::spawn_blocking(move || render_report(&data, font.as_deref()))
        .await
        .map_err(|_| AppError::internal())?
        .map_err(|err| {
            tracing::error!("{err}");
            AppError::internal()
        })?;
    let filename = format!("report-{}-{}_{}.pdf", req.report_type, req.period_from, req.period_to);
    crate::audit!(Some(actor.id), "report_generated", "type": req.report_type, "period_from": req.period_from,
                  "period_to": req.period_to, "rows": rows);
    Ok((pdf, filename))
}

// --- по документам ------------------------------------------------------------------------------------

fn fill_documents(state: &AppState, data: &mut ReportData, docs: &[Document], executors: &HashMap<Uuid, Vec<UserBrief>>) {
    let today = state.settings.business_today();
    let warning_days = state.settings.deadline_warning_days;
    data.columns = ["Рег. номер", "Дата рег.", "Корреспондент", "Тип", "Краткое содержание", "Срок", "Статус", "Исполнители"]
        .map(String::from)
        .to_vec();
    data.col_weights = vec![1.7, 1.1, 2.2, 1.5, 4.2, 1.1, 1.5, 2.2];
    // счетчик по статусам в порядке первого появления
    let mut by_status: Vec<(DocumentStatus, usize)> = Vec::new();
    let (mut overdue, mut warning) = (0, 0);
    for d in docs {
        let state = deadline_state(d.execution_deadline, d.status, today, warning_days);
        overdue += usize::from(state == DeadlineState::Overdue);
        warning += usize::from(state == DeadlineState::Warning);
        match by_status.iter_mut().find(|(s, _)| *s == d.status) {
            Some((_, n)) => *n += 1,
            None => by_status.push((d.status, 1)),
        }
        let status_text =
            format!("{}{}", d.status, if state == DeadlineState::Overdue { " (просрочен)" } else { "" });
        data.rows.push(vec![
            d.registration_number.clone(),
            fmt_date(d.registration_date),
            d.correspondent_name.clone(),
            d.document_type_name.clone(),
            shorten(&d.summary, 220),
            fmt_date(d.execution_deadline),
            status_text,
            executor_names(executors.get(&d.id), "—"),
        ]);
    }
    data.summary.push(("Всего документов".into(), docs.len().to_string()));
    for (status, n) in by_status {
        data.summary.push((format!("Статус «{status}»"), n.to_string()));
    }
    data.summary.push(("Просрочено".into(), overdue.to_string()));
    data.summary.push((format!("Срок наступает в ближайшие {warning_days} дн."), warning.to_string()));
}

// --- по исполнителям ----------------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct ResolutionReportRow {
    assigned_executor_id: Uuid,
    executor_name: String,
    document_id: Uuid,
    created_at: DateTime<Utc>,
    executed_at: Option<DateTime<Utc>>,
    deadline: NaiveDate,
}

async fn fill_executors(state: &AppState, data: &mut ReportData, req: &ReportRequest) -> AppResult<()> {
    let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "SELECT r.assigned_executor_id, u.full_name AS executor_name, r.document_id, r.created_at, r.executed_at, \
         r.deadline FROM resolutions r JOIN incoming_documents d ON d.id = r.document_id \
         JOIN users u ON u.id = r.assigned_executor_id WHERE d.registration_date >= ",
    );
    qb.push_bind(req.period_from).push(" AND d.registration_date <= ").push_bind(req.period_to);
    if let Some(id) = req.executor_id {
        qb.push(" AND r.assigned_executor_id = ").push_bind(id);
    }
    if let Some(id) = req.correspondent_id {
        qb.push(" AND d.correspondent_id = ").push_bind(id);
    }
    if let Some(status) = req.status {
        qb.push(" AND d.status = ").push_bind(status);
    }
    if let Some(id) = req.document_type_id {
        qb.push(" AND d.document_type_id = ").push_bind(id);
    }
    qb.push(" ORDER BY r.created_at, r.id");
    let rows: Vec<ResolutionReportRow> = qb.build_query_as().fetch_all(&state.db).await?;

    let tz = state.settings.timezone;
    let facts: Vec<ResolutionFact> = rows
        .into_iter()
        .map(|r| ResolutionFact {
            executor_id: r.assigned_executor_id.to_string(),
            executor_name: r.executor_name,
            document_id: r.document_id.to_string(),
            created_at: r.created_at.with_timezone(&tz),
            executed_at: r.executed_at.map(|e| e.with_timezone(&tz)),
            deadline: r.deadline,
        })
        .collect();
    let stats = compute_executor_stats(&facts, state.settings.business_today());
    data.columns = ["ФИО исполнителя", "Документов", "Резолюций", "Исполнено", "Средний срок исполнения, дн.",
                    "Просрочено", "Доля просрочки, %"]
        .map(String::from)
        .to_vec();
    data.col_weights = vec![4.0, 1.3, 1.3, 1.3, 2.0, 1.3, 1.6];
    for s in &stats {
        data.rows.push(vec![
            s.executor_name.clone(),
            s.documents.to_string(),
            s.resolutions.to_string(),
            s.executed.to_string(),
            s.avg_days.map_or_else(|| "—".to_string(), |v| format!("{v:.1}")),
            s.overdue.to_string(),
            format!("{:.1}", s.overdue_pct),
        ]);
    }
    let total: usize = stats.iter().map(|s| s.resolutions).sum();
    let overdue: usize = stats.iter().map(|s| s.overdue).sum();
    data.summary = vec![
        ("Исполнителей".into(), stats.len().to_string()),
        ("Резолюций всего".into(), total.to_string()),
        ("Из них просрочено".into(), overdue.to_string()),
        (
            "Общая доля просрочки, %".into(),
            if total > 0 { format!("{:.1}", overdue as f64 * 100.0 / total as f64) } else { "0.0".into() },
        ),
    ];
    Ok(())
}

// --- по срокам исполнения -----------------------------------------------------------------------------

async fn fill_deadlines(state: &AppState, data: &mut ReportData, docs: &[Document]) -> AppResult<()> {
    let tz = state.settings.timezone;
    let today = state.settings.business_today();
    let ids: Vec<Uuid> = docs.iter().map(|d| d.id).collect();
    // момент последнего перехода документа в статус «исполнен» (по истории)
    let moments: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT document_id, max(created_at) FROM document_history \
         WHERE document_id = ANY($1) AND new_status = $2 GROUP BY document_id",
    )
    .bind(&ids)
    .bind(DocumentStatus::Executed)
    .fetch_all(&state.db)
    .await?;
    let executed: HashMap<Uuid, NaiveDate> =
        moments.into_iter().map(|(id, at)| (id, at.with_timezone(&tz).date_naive())).collect();

    data.columns = ["Рег. номер", "Корреспондент", "Срок исполнения", "Фактическая дата исполнения", "Результат",
                    "Отклонение, дн."]
        .map(String::from)
        .to_vec();
    data.col_weights = vec![1.8, 3.0, 1.5, 1.8, 3.0, 1.4];
    let mut counts: BTreeMap<DeadlineCategory, usize> = BTreeMap::new();
    for d in docs {
        let executed_date = executed.get(&d.id).copied();
        let (category, deviation) = classify_deadline(d.execution_deadline, d.status, executed_date, today);
        *counts.entry(category).or_default() += 1;
        let is_done = matches!(category, DeadlineCategory::OnTime | DeadlineCategory::Late);
        data.rows.push(vec![
            d.registration_number.clone(),
            d.correspondent_name.clone(),
            fmt_date(d.execution_deadline),
            if is_done { executed_date.map_or_else(|| "—".into(), fmt_date) } else { "—".into() },
            category.label().into(),
            deviation.map_or_else(|| "—".into(), |v| format!("{v:+}")),
        ]);
    }
    data.summary.push(("Всего документов".into(), docs.len().to_string()));
    for category in DeadlineCategory::ALL {
        if let Some(n) = counts.get(&category).filter(|n| **n > 0) {
            data.summary.push((category.label().into(), n.to_string()));
        }
    }
    let on_time = counts.get(&DeadlineCategory::OnTime).copied().unwrap_or(0);
    let done = on_time + counts.get(&DeadlineCategory::Late).copied().unwrap_or(0);
    if done > 0 {
        data.summary.push(("Доля исполненных в срок, %".into(), format!("{:.1}", on_time as f64 * 100.0 / done as f64)));
    }
    Ok(())
}
