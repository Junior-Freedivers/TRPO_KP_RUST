//! Бизнес-логика входящих документов: регистрация, редактирование, статусы, удаление, поиск.

use std::collections::HashMap;

use chrono::{Duration, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, PgExecutor, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::audit::{self, DocumentEvent, document_event};
use crate::domain::deadlines::{days_left, deadline_state};
use crate::domain::reg_number::{counter_key, format_registration_number};
use crate::domain::workflow::{CLOSED_STATUSES, transition_roles};
use crate::dto::{
    AttachmentOut, CorrespondentBrief, DocumentCreate, DocumentDetail, DocumentFilters, DocumentOut, DocumentUpdate,
    HistoryOut, Jsonable, NamedBrief, Page, ResolutionOut, StatusChangeRequest, UserBrief,
};
use crate::error::{AppError, AppResult};
use crate::models::{
    ATTACHMENT_SELECT, Attachment, DOCUMENT_SELECT, DeadlineState, Document, DocumentStatus, HISTORY_SELECT,
    HistoryEntry, RESOLUTION_SELECT, Resolution, ResolutionStatus, Role, User,
};
use crate::security::{Permission, has_permission};
use crate::services::like_pattern;
use crate::state::AppState;

fn not_found() -> AppError {
    AppError::not_found("Документ не найден")
}

// --- чтение из БД -----------------------------------------------------------------------------------

pub async fn find_document<'e>(db: impl PgExecutor<'e>, id: Uuid) -> Result<Option<Document>, sqlx::Error> {
    sqlx::query_as(&format!("{DOCUMENT_SELECT} WHERE d.id = $1")).bind(id).fetch_optional(db).await
}

/// Блокировка строки документа (SELECT ... FOR UPDATE) на время транзакции.
pub async fn lock_document(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<Uuid> = sqlx::query_scalar("SELECT id FROM incoming_documents WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(row.is_some())
}

pub async fn load_resolutions<'e>(
    db: impl PgExecutor<'e>,
    document_id: Uuid,
    executor_id: Option<Uuid>,
) -> Result<Vec<Resolution>, sqlx::Error> {
    let mut qb = QueryBuilder::<Postgres>::new(RESOLUTION_SELECT);
    qb.push(" WHERE r.document_id = ").push_bind(document_id);
    if let Some(executor) = executor_id {
        qb.push(" AND r.assigned_executor_id = ").push_bind(executor);
    }
    qb.push(" ORDER BY r.created_at, r.id");
    qb.build_query_as().fetch_all(db).await
}

/// Исполнители документов (в порядке создания резолюций, без повторов).
pub async fn load_executors<'e>(
    db: impl PgExecutor<'e>,
    document_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<UserBrief>>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT r.document_id, u.id, u.full_name FROM resolutions r JOIN users u ON u.id = r.assigned_executor_id \
         WHERE r.document_id = ANY($1) ORDER BY r.created_at, r.id",
    )
    .bind(document_ids)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<Uuid, Vec<UserBrief>> = HashMap::new();
    for (doc_id, user_id, full_name) in rows {
        let list = map.entry(doc_id).or_default();
        if !list.iter().any(|u| u.id == user_id) {
            list.push(UserBrief { id: user_id, full_name });
        }
    }
    Ok(map)
}

pub async fn is_assigned<'e>(db: impl PgExecutor<'e>, document_id: Uuid, user_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resolutions WHERE document_id = $1 AND assigned_executor_id = $2)",
    )
    .bind(document_id)
    .bind(user_id)
    .fetch_one(db)
    .await
}

/// Исполнитель видит только документы, по которым ему назначены резолюции.
pub async fn assert_can_view<'e>(db: impl PgExecutor<'e>, actor: &User, document_id: Uuid) -> AppResult<()> {
    if actor.role == Role::Executor && !is_assigned(db, document_id, actor.id).await? {
        return Err(AppError::forbidden("Документ не назначен вам на исполнение"));
    }
    Ok(())
}

// --- преобразование в схемы ответа --------------------------------------------------------------------

pub fn to_out(doc: &Document, executors: Vec<UserBrief>, actor: Option<&User>, today: NaiveDate, warning_days: i64) -> DocumentOut {
    let state = deadline_state(doc.execution_deadline, doc.status, today, warning_days);
    let executors = match actor {
        Some(a) if a.role == Role::Executor => executors.into_iter().filter(|u| u.id == a.id).collect(),
        _ => executors,
    };
    DocumentOut {
        id: doc.id,
        registration_number: doc.registration_number.clone(),
        received_date: doc.received_date,
        registration_date: doc.registration_date,
        correspondent: CorrespondentBrief {
            id: doc.correspondent_id,
            name: doc.correspondent_name.clone(),
            inn: doc.correspondent_inn.clone(),
        },
        addressee: doc.addressee.clone(),
        summary: doc.summary.clone(),
        document_type: NamedBrief { id: doc.document_type_id, name: doc.document_type_name.clone() },
        page_count: doc.page_count,
        execution_deadline: doc.execution_deadline,
        status: doc.status,
        created_by: doc.created_by,
        created_at: doc.created_at,
        updated_at: doc.updated_at,
        archived_at: doc.archived_at,
        deadline_state: state,
        is_overdue: state == DeadlineState::Overdue,
        days_left: (state != DeadlineState::Closed).then(|| days_left(doc.execution_deadline, today)),
        executors,
    }
}

async fn to_detail(state: &AppState, actor: &User, doc: Document) -> AppResult<DocumentDetail> {
    let resolutions = load_resolutions(&state.db, doc.id, None).await?;
    let mut executors: Vec<UserBrief> = Vec::new();
    for r in &resolutions {
        if !executors.iter().any(|u| u.id == r.assigned_executor_id) {
            executors.push(UserBrief { id: r.assigned_executor_id, full_name: r.executor_name.clone() });
        }
    }
    let attachments: Vec<Attachment> =
        sqlx::query_as(&format!("{ATTACHMENT_SELECT} WHERE f.document_id = $1 ORDER BY f.created_at, f.id"))
            .bind(doc.id)
            .fetch_all(&state.db)
            .await?;
    let history = load_history(state, doc.id).await?;

    let today = state.settings.business_today();
    let document = to_out(&doc, executors, Some(actor), today, state.settings.deadline_warning_days);
    let resolutions = resolutions
        .iter()
        .filter(|r| actor.role != Role::Executor || r.assigned_executor_id == actor.id)
        .map(ResolutionOut::from)
        .collect();
    Ok(DocumentDetail {
        document,
        resolutions,
        attachments: attachments.iter().map(AttachmentOut::from).collect(),
        history,
    })
}

async fn load_history(state: &AppState, document_id: Uuid) -> AppResult<Vec<HistoryOut>> {
    let entries: Vec<HistoryEntry> =
        sqlx::query_as(&format!("{HISTORY_SELECT} WHERE h.document_id = $1 ORDER BY h.created_at, h.seq"))
            .bind(document_id)
            .fetch_all(&state.db)
            .await?;
    Ok(entries.into_iter().map(Into::into).collect())
}

async fn detail_by_id(state: &AppState, actor: &User, id: Uuid) -> AppResult<DocumentDetail> {
    let doc = find_document(&state.db, id).await?.ok_or_else(not_found)?;
    to_detail(state, actor, doc).await
}

// --- чтение -----------------------------------------------------------------------------------------

pub async fn get_detail(state: &AppState, actor: &User, id: Uuid) -> AppResult<DocumentDetail> {
    let doc = find_document(&state.db, id).await?.ok_or_else(not_found)?;
    assert_can_view(&state.db, actor, doc.id).await?;
    to_detail(state, actor, doc).await
}

pub async fn get_by_number(state: &AppState, actor: &User, number: &str) -> AppResult<DocumentDetail> {
    let doc: Document = sqlx::query_as(&format!("{DOCUMENT_SELECT} WHERE d.registration_number = $1"))
        .bind(number)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("Документ с таким регистрационным номером не найден"))?;
    assert_can_view(&state.db, actor, doc.id).await?;
    to_detail(state, actor, doc).await
}

pub async fn history(state: &AppState, actor: &User, id: Uuid) -> AppResult<Vec<HistoryOut>> {
    let doc = find_document(&state.db, id).await?.ok_or_else(not_found)?;
    assert_can_view(&state.db, actor, doc.id).await?;
    load_history(state, doc.id).await
}

fn push_filters(
    qb: &mut QueryBuilder<'_, Postgres>,
    f: &DocumentFilters,
    executor_scope: Option<Uuid>,
    today: NaiveDate,
    warning_days: i64,
) {
    qb.push(" WHERE true");
    if let Some(scope) = executor_scope {
        qb.push(" AND d.id IN (SELECT document_id FROM resolutions WHERE assigned_executor_id = ")
            .push_bind(scope)
            .push(")");
    }
    if let Some(number) = f.registration_number.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND d.registration_number ILIKE ").push_bind(like_pattern(number)).push(" ESCAPE '\\'");
    }
    if let Some(from) = f.date_from {
        qb.push(" AND d.registration_date >= ").push_bind(from);
    }
    if let Some(to) = f.date_to {
        qb.push(" AND d.registration_date <= ").push_bind(to);
    }
    if let Some(id) = f.correspondent_id {
        qb.push(" AND d.correspondent_id = ").push_bind(id);
    }
    if let Some(id) = f.executor_id {
        qb.push(" AND d.id IN (SELECT document_id FROM resolutions WHERE assigned_executor_id = ")
            .push_bind(id)
            .push(")");
    }
    if let Some(status) = f.status {
        qb.push(" AND d.status = ").push_bind(status);
    }
    if let Some(id) = f.document_type_id {
        qb.push(" AND d.document_type_id = ").push_bind(id);
    }
    if let Some(search) = f.search.as_deref().filter(|s| !s.is_empty()) {
        let pattern = like_pattern(search);
        qb.push(" AND (d.summary ILIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\'");
        qb.push(" OR d.addressee ILIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\'");
        qb.push(" OR d.registration_number ILIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\'");
        qb.push(" OR d.correspondent_id IN (SELECT id FROM correspondents WHERE name ILIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '\\'))");
    }
    if let Some(state) = f.deadline_state {
        let closed: Vec<&str> = CLOSED_STATUSES.iter().map(|s| s.as_str()).collect();
        match state {
            DeadlineState::Overdue => {
                qb.push(" AND d.status <> ALL(").push_bind(closed).push(")");
                qb.push(" AND d.execution_deadline < ").push_bind(today);
            }
            DeadlineState::Warning => {
                qb.push(" AND d.status <> ALL(").push_bind(closed).push(")");
                qb.push(" AND d.execution_deadline >= ").push_bind(today);
                qb.push(" AND d.execution_deadline <= ").push_bind(today + Duration::days(warning_days));
            }
            _ => {}
        }
    }
}

fn push_order(qb: &mut QueryBuilder<'_, Postgres>, sort: &str) {
    qb.push(" ORDER BY ");
    for token in sort.split(',') {
        let token = token.trim();
        let column = token.trim_start_matches('-');
        // имя столбца проверено по белому списку DOCUMENT_SORT_FIELDS при валидации фильтров
        qb.push(format!("d.{column}{}, ", if token.starts_with('-') { " DESC" } else { "" }));
    }
    qb.push("d.created_at DESC, d.id"); // стабильный порядок для пагинации
}

pub async fn search(state: &AppState, actor: &User, filters: DocumentFilters) -> AppResult<Page<DocumentOut>> {
    let (page, page_size, sort) = filters.validate()?;
    // матрица прав: без DOCUMENT_SEARCH доступен только список (пагинация, сортировка) без фильтров
    if filters.has_search_filters() && !has_permission(actor.role, Permission::DocumentSearch) {
        return Err(AppError::forbidden("Поиск и фильтрация недоступны для вашей роли"));
    }
    let scope = (actor.role == Role::Executor).then_some(actor.id);
    let today = state.settings.business_today();
    let warning_days = state.settings.deadline_warning_days;

    let mut count = QueryBuilder::new("SELECT count(*) FROM incoming_documents d");
    push_filters(&mut count, &filters, scope, today, warning_days);
    let total: i64 = count.build_query_scalar().fetch_one(&state.db).await?;

    let mut select = QueryBuilder::new(DOCUMENT_SELECT);
    push_filters(&mut select, &filters, scope, today, warning_days);
    push_order(&mut select, &sort);
    select.push(" LIMIT ").push_bind(page_size).push(" OFFSET ").push_bind((page - 1) * page_size);
    let docs: Vec<Document> = select.build_query_as().fetch_all(&state.db).await?;

    let ids: Vec<Uuid> = docs.iter().map(|d| d.id).collect();
    let mut executors = load_executors(&state.db, &ids).await?;
    let items = docs
        .iter()
        .map(|d| to_out(d, executors.remove(&d.id).unwrap_or_default(), Some(actor), today, warning_days))
        .collect();
    Ok(Page::new(items, total, page, page_size))
}

/// Все документы по фильтрам без пагинации (отчеты и выгрузки) + исполнители каждого документа.
pub async fn search_all(
    state: &AppState,
    filters: &DocumentFilters,
) -> AppResult<(Vec<Document>, HashMap<Uuid, Vec<UserBrief>>)> {
    let today = state.settings.business_today();
    let mut select = QueryBuilder::new(DOCUMENT_SELECT);
    push_filters(&mut select, filters, None, today, state.settings.deadline_warning_days);
    select.push(" ORDER BY d.registration_date, d.registration_number");
    let docs: Vec<Document> = select.build_query_as().fetch_all(&state.db).await?;
    let ids: Vec<Uuid> = docs.iter().map(|d| d.id).collect();
    let executors = load_executors(&state.db, &ids).await?;
    Ok((docs, executors))
}

// --- проверки ---------------------------------------------------------------------------------------

async fn require_correspondent<'e>(db: impl PgExecutor<'e>, id: Uuid) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM correspondents WHERE id = $1)")
        .bind(id)
        .fetch_one(db)
        .await?;
    if !exists {
        return Err(AppError::validation("Корреспондент не найден").with_field("correspondent_id"));
    }
    Ok(())
}

async fn require_document_type<'e>(db: impl PgExecutor<'e>, id: Uuid) -> AppResult<()> {
    let active: Option<bool> = sqlx::query_scalar("SELECT is_active FROM document_types WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    match active {
        None => Err(AppError::validation("Тип документа не найден").with_field("document_type_id")),
        Some(false) => Err(AppError::validation("Тип документа выведен из использования").with_field("document_type_id")),
        Some(true) => Ok(()),
    }
}

fn check_dates(received: NaiveDate, deadline: NaiveDate, registration_date: NaiveDate) -> AppResult<()> {
    if received > registration_date {
        return Err(AppError::validation("Дата поступления не может быть позже даты регистрации")
            .with_field("received_date"));
    }
    if deadline < registration_date {
        return Err(AppError::validation("Срок исполнения не может быть раньше даты регистрации")
            .with_field("execution_deadline"));
    }
    Ok(())
}

/// Атомарный инкремент счетчика: INSERT .. ON CONFLICT DO UPDATE блокирует строку счетчика до конца
/// транзакции, поэтому параллельные регистрации получают разные номера без гонок.
pub async fn next_sequence(conn: &mut PgConnection, key: i32) -> Result<i64, sqlx::Error> {
    let value: i32 = sqlx::query_scalar(
        "INSERT INTO registration_counters (year, last_value) VALUES ($1, 1) \
         ON CONFLICT (year) DO UPDATE SET last_value = registration_counters.last_value + 1 RETURNING last_value",
    )
    .bind(key)
    .fetch_one(conn)
    .await?;
    Ok(value as i64)
}

// --- регистрация ------------------------------------------------------------------------------------

pub async fn register(state: &AppState, actor: &User, data: DocumentCreate) -> AppResult<DocumentDetail> {
    require_correspondent(&state.db, data.correspondent_id).await?;
    require_document_type(&state.db, data.document_type_id).await?;
    let today = state.settings.business_today();
    check_dates(data.received_date, data.execution_deadline, today)?;

    let template = &state.settings.reg_number_template;
    let mut tx = state.db.begin().await?;
    let sequence = next_sequence(&mut tx, counter_key(template, chrono::Datelike::year(&today))).await?;
    let number = format_registration_number(template, chrono::Datelike::year(&today), sequence);
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incoming_documents (id, registration_number, received_date, registration_date, correspondent_id, \
         addressee, summary, document_type_id, page_count, execution_deadline, status, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(&number)
    .bind(data.received_date)
    .bind(today)
    .bind(data.correspondent_id)
    .bind(&data.addressee)
    .bind(data.summary.trim())
    .bind(data.document_type_id)
    .bind(data.page_count as i32)
    .bind(data.execution_deadline)
    .bind(DocumentStatus::Registered)
    .bind(actor.id)
    .execute(&mut *tx)
    .await?;
    document_event(&mut tx, id, actor.id, "document_registered", DocumentEvent {
        new_status: Some(DocumentStatus::Registered),
        meta: Some(json!({"registration_number": number})),
        ..Default::default()
    })
    .await?;
    tx.commit().await?;
    crate::audit!(Some(actor.id), "document_registered", "document_id": id, "registration_number": number);
    detail_by_id(state, actor, id).await
}

// --- редактирование ---------------------------------------------------------------------------------

fn track<T: PartialEq + Jsonable>(diff: &mut Map<String, Value>, name: &str, old: &T, new: &T) -> bool {
    if old == new {
        return false;
    }
    diff.insert(name.to_string(), json!({"old": old.to_json(), "new": new.to_json()}));
    true
}

pub async fn update(state: &AppState, actor: &User, id: Uuid, data: DocumentUpdate) -> AppResult<DocumentDetail> {
    let mut tx = state.db.begin().await?;
    if !lock_document(&mut tx, id).await? {
        return Err(not_found());
    }
    let doc = find_document(&mut *tx, id).await?.ok_or_else(not_found)?;
    if doc.status == DocumentStatus::Archived {
        return Err(AppError::conflict("Архивный документ не может быть изменен").with_code("DOCUMENT_ARCHIVED"));
    }

    let addressee_set = data.addressee.is_set();
    let addressee = data.addressee.as_value().cloned();
    let received_date = data.received_date.required("received_date")?;
    let correspondent_id = data.correspondent_id.required("correspondent_id")?;
    let summary = data.summary.required("summary")?;
    let document_type_id = data.document_type_id.required("document_type_id")?;
    let page_count = data.page_count.required("page_count")?;
    let execution_deadline = data.execution_deadline.required("execution_deadline")?;

    if let Some(c) = correspondent_id
        && c != doc.correspondent_id
    {
        require_correspondent(&mut *tx, c).await?;
    }
    if let Some(t) = document_type_id
        && t != doc.document_type_id
    {
        require_document_type(&mut *tx, t).await?;
    }
    check_dates(
        received_date.unwrap_or(doc.received_date),
        execution_deadline.unwrap_or(doc.execution_deadline),
        doc.registration_date,
    )?;

    let mut diff = Map::new();
    let mut qb = QueryBuilder::<Postgres>::new("UPDATE incoming_documents SET updated_at = now()");
    if let Some(v) = received_date
        && track(&mut diff, "received_date", &doc.received_date, &v)
    {
        qb.push(", received_date = ").push_bind(v);
    }
    if let Some(v) = correspondent_id
        && track(&mut diff, "correspondent_id", &doc.correspondent_id, &v)
    {
        qb.push(", correspondent_id = ").push_bind(v);
    }
    if addressee_set && track(&mut diff, "addressee", &doc.addressee, &addressee) {
        qb.push(", addressee = ").push_bind(addressee);
    }
    if let Some(v) = summary
        && track(&mut diff, "summary", &doc.summary, &v)
    {
        qb.push(", summary = ").push_bind(v);
    }
    if let Some(v) = document_type_id
        && track(&mut diff, "document_type_id", &doc.document_type_id, &v)
    {
        qb.push(", document_type_id = ").push_bind(v);
    }
    if let Some(v) = page_count
        && track(&mut diff, "page_count", &(doc.page_count as i64), &v)
    {
        qb.push(", page_count = ").push_bind(v as i32);
    }
    if let Some(v) = execution_deadline
        && track(&mut diff, "execution_deadline", &doc.execution_deadline, &v)
    {
        qb.push(", execution_deadline = ").push_bind(v);
    }

    if !diff.is_empty() {
        qb.push(" WHERE id = ").push_bind(id);
        qb.build().execute(&mut *tx).await?;
        let mut fields: Vec<String> = diff.keys().cloned().collect();
        fields.sort();
        document_event(&mut tx, id, actor.id, "document_updated", DocumentEvent {
            meta: Some(json!({"changes": diff})),
            ..Default::default()
        })
        .await?;
        tx.commit().await?;
        crate::audit!(Some(actor.id), "document_updated", "document_id": id, "fields": fields);
    } else {
        tx.rollback().await?;
    }
    detail_by_id(state, actor, id).await
}

// --- смена статуса ----------------------------------------------------------------------------------

pub async fn change_status(state: &AppState, actor: &User, id: Uuid, data: StatusChangeRequest) -> AppResult<DocumentDetail> {
    let mut tx = state.db.begin().await?;
    if !lock_document(&mut tx, id).await? {
        return Err(not_found());
    }
    let doc = find_document(&mut *tx, id).await?.ok_or_else(not_found)?;
    assert_can_view(&mut *tx, actor, doc.id).await?;

    let (old, new) = (doc.status, data.status);
    let Some(allowed_roles) = transition_roles(old, new) else {
        return Err(AppError::conflict(format!("Переход «{old}» → «{new}» недопустим"))
            .with_code("INVALID_STATUS_TRANSITION")
            .with_field("status"));
    };
    if !allowed_roles.contains(&actor.role) {
        return Err(AppError::forbidden("Ваша роль не может выполнить этот переход статуса"));
    }

    let comment = data.comment.as_deref();
    if actor.role == Role::Executor {
        complete_as_executor(&mut tx, actor, &doc, comment).await?;
    } else {
        let archived_at = (new == DocumentStatus::Archived).then(Utc::now);
        sqlx::query("UPDATE incoming_documents SET status = $1, archived_at = $2, updated_at = now() WHERE id = $3")
            .bind(new)
            .bind(archived_at)
            .bind(doc.id)
            .execute(&mut *tx)
            .await?;
        let action = if new == DocumentStatus::Registered { "document_annulled" } else { "status_changed" };
        document_event(&mut tx, doc.id, actor.id, action, DocumentEvent {
            old_status: Some(old),
            new_status: Some(new),
            comment,
            meta: None,
        })
        .await?;
    }
    tx.commit().await?;
    crate::audit!(Some(actor.id), "status_change", "document_id": doc.id, "old": old, "requested": new);
    detail_by_id(state, actor, doc.id).await
}

/// Исполнитель закрывает свои резолюции; документ становится «исполнен», когда закрыты все.
async fn complete_as_executor(conn: &mut PgConnection, actor: &User, doc: &Document, comment: Option<&str>) -> AppResult<()> {
    let mine = load_resolutions(&mut *conn, doc.id, Some(actor.id)).await?;
    if mine.is_empty() {
        return Err(AppError::forbidden("Документ не назначен вам на исполнение"));
    }
    let active: Vec<Uuid> = mine.iter().filter(|r| r.status == ResolutionStatus::InProgress).map(|r| r.id).collect();
    if active.is_empty() {
        return Err(AppError::conflict("Ваши резолюции по этому документу уже исполнены")
            .with_code("RESOLUTION_ALREADY_EXECUTED"));
    }
    sqlx::query("UPDATE resolutions SET status = $1, executed_at = now(), updated_at = now() WHERE id = ANY($2)")
        .bind(ResolutionStatus::Executed)
        .bind(&active)
        .execute(&mut *conn)
        .await?;
    let ids: Vec<String> = active.iter().map(Uuid::to_string).collect();
    document_event(&mut *conn, doc.id, actor.id, "resolution_executed", DocumentEvent {
        comment,
        meta: Some(json!({"resolution_ids": ids})),
        ..Default::default()
    })
    .await?;
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM resolutions WHERE document_id = $1 AND status = $2")
        .bind(doc.id)
        .bind(ResolutionStatus::InProgress)
        .fetch_one(&mut *conn)
        .await?;
    if remaining == 0 {
        sqlx::query("UPDATE incoming_documents SET status = $1, updated_at = now() WHERE id = $2")
            .bind(DocumentStatus::Executed)
            .bind(doc.id)
            .execute(&mut *conn)
            .await?;
        document_event(&mut *conn, doc.id, actor.id, "status_changed", DocumentEvent {
            old_status: Some(DocumentStatus::InExecution),
            new_status: Some(DocumentStatus::Executed),
            comment,
            meta: None,
        })
        .await?;
    }
    Ok(())
}

// --- удаление ---------------------------------------------------------------------------------------

pub async fn delete(state: &AppState, actor: &User, id: Uuid) -> AppResult<()> {
    let doc = find_document(&state.db, id).await?.ok_or_else(not_found)?;
    let mut tx = state.db.begin().await?;
    let keys: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM attachments WHERE document_id = $1")
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    // дочерние записи (резолюции, вложения, история) удаляются каскадом на уровне БД
    sqlx::query("DELETE FROM incoming_documents WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    for key in &keys {
        // после успешного commit; сбой удаления файла не откатывает удаление карточки
        if let Err(err) = state.storage.delete(key).await {
            tracing::warn!("Не удалось удалить объект хранилища {key}: {err}");
        }
    }
    audit::log_action(
        Some(actor.id),
        "document_deleted",
        json!({"document_id": id, "registration_number": doc.registration_number, "files": keys.len()}),
    );
    Ok(())
}
