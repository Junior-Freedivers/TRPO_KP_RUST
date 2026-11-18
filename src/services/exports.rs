//! Выгрузка журнала регистрации (CSV).

use crate::domain::deadlines::deadline_state;
use crate::dto::{DocumentFilters, RegistrationLogQuery};
use crate::error::AppResult;
use crate::models::{DeadlineState, User};
use crate::reports::csv::{CsvValue, render_csv};
use crate::services::documents::search_all;
use crate::services::reports::{collapse_whitespace, executor_names, fmt_date};
use crate::state::AppState;

const HEADERS: [&str; 13] = [
    "Регистрационный номер", "Дата поступления", "Дата регистрации", "Корреспондент", "ИНН корреспондента",
    "Адресат", "Краткое содержание", "Тип документа", "Количество листов", "Срок исполнения", "Статус",
    "Исполнители", "Просрочен",
];

pub async fn registration_log(state: &AppState, actor: &User, query: RegistrationLogQuery) -> AppResult<Vec<u8>> {
    query.validate()?;
    let today = state.settings.business_today();
    let warning_days = state.settings.deadline_warning_days;
    let filters = DocumentFilters {
        date_from: query.date_from,
        date_to: query.date_to,
        correspondent_id: query.correspondent_id,
        status: query.status,
        document_type_id: query.document_type_id,
        ..Default::default()
    };
    let (docs, executors) = search_all(state, &filters).await?;
    let rows: Vec<Vec<CsvValue>> = docs
        .iter()
        .map(|d| {
            let overdue = deadline_state(d.execution_deadline, d.status, today, warning_days) == DeadlineState::Overdue;
            vec![
                d.registration_number.clone().into(),
                fmt_date(d.received_date).into(),
                fmt_date(d.registration_date).into(),
                d.correspondent_name.clone().into(),
                d.correspondent_inn.clone().unwrap_or_default().into(),
                d.addressee.clone().unwrap_or_default().into(),
                collapse_whitespace(&d.summary).into(),
                d.document_type_name.clone().into(),
                (d.page_count as i64).into(),
                fmt_date(d.execution_deadline).into(),
                d.status.as_str().into(),
                executor_names(executors.get(&d.id), "").into(),
                (if overdue { "да" } else { "нет" }).into(),
            ]
        })
        .collect();
    crate::audit!(Some(actor.id), "registration_log_exported", "rows": rows.len(),
                  "date_from": query.date_from, "date_to": query.date_to);
    Ok(render_csv(&HEADERS, &rows))
}
