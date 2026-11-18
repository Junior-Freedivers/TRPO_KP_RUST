//! Корреспонденты (делопроизводитель).

use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::audit;
use crate::dto::{CorrespondentCreate, CorrespondentListQuery, CorrespondentOut, CorrespondentUpdate, Page, Patch, page_params};
use crate::error::{AppError, AppResult};
use crate::models::{Correspondent, User};
use crate::services::like_pattern;
use crate::state::AppState;

const SELECT: &str =
    "SELECT id, name, inn, address, phone, email, signer_full_name, created_at, updated_at FROM correspondents";

pub async fn get(state: &AppState, id: Uuid) -> AppResult<CorrespondentOut> {
    let item: Option<Correspondent> =
        sqlx::query_as(&format!("{SELECT} WHERE id = $1")).bind(id).fetch_optional(&state.db).await?;
    item.map(Into::into).ok_or_else(|| AppError::not_found("Корреспондент не найден"))
}

pub async fn create(state: &AppState, actor: &User, data: CorrespondentCreate) -> AppResult<CorrespondentOut> {
    let id = Uuid::new_v4();
    let name = data.name.trim().to_string();
    sqlx::query(
        "INSERT INTO correspondents (id, name, inn, address, phone, email, signer_full_name) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(&name)
    .bind(data.inn)
    .bind(data.address)
    .bind(data.phone)
    .bind(data.email)
    .bind(data.signer_full_name)
    .execute(&state.db)
    .await?;
    audit!(Some(actor.id), "correspondent_created", "correspondent_id": id, "name": name);
    get(state, id).await
}

pub async fn list_page(state: &AppState, q: CorrespondentListQuery) -> AppResult<Page<CorrespondentOut>> {
    let (page, page_size) = page_params(q.page, q.page_size)?;
    if q.search.as_ref().is_some_and(|s| s.chars().count() > 200) {
        return Err(AppError::invalid_field("search", "Строка должна содержать не более 200 символов"));
    }
    if q.inn.as_ref().is_some_and(|s| s.chars().count() > 12) {
        return Err(AppError::invalid_field("inn", "Строка должна содержать не более 12 символов"));
    }
    let filters = |qb: &mut QueryBuilder<'_, Postgres>| {
        qb.push(" WHERE true");
        if let Some(search) = q.search.as_deref().filter(|s| !s.is_empty()) {
            let pattern = like_pattern(search);
            qb.push(" AND (name ILIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\'");
            qb.push(" OR signer_full_name ILIKE ").push_bind(pattern).push(" ESCAPE '\\')");
        }
        if let Some(inn) = q.inn.as_deref().filter(|s| !s.is_empty()) {
            qb.push(" AND inn = ").push_bind(inn.to_string());
        }
    };
    let mut count = QueryBuilder::new("SELECT count(*) FROM correspondents");
    filters(&mut count);
    let total: i64 = count.build_query_scalar().fetch_one(&state.db).await?;

    let mut select = QueryBuilder::new(SELECT);
    filters(&mut select);
    select.push(" ORDER BY name, id LIMIT ").push_bind(page_size).push(" OFFSET ").push_bind((page - 1) * page_size);
    let items: Vec<Correspondent> = select.build_query_as().fetch_all(&state.db).await?;
    Ok(Page::new(items.into_iter().map(Into::into).collect(), total, page, page_size))
}

pub async fn update(state: &AppState, actor: &User, id: Uuid, data: CorrespondentUpdate) -> AppResult<CorrespondentOut> {
    get(state, id).await?;
    if data.name == Patch::Null {
        return Err(AppError::validation("Наименование не может быть пустым").with_field("name"));
    }
    let mut fields: Vec<&str> = Vec::new();
    let mut qb = QueryBuilder::<Postgres>::new("UPDATE correspondents SET updated_at = now()");
    if let Patch::Value(name) = data.name {
        qb.push(", name = ").push_bind(name);
        fields.push("name");
    }
    for (column, value) in [
        ("inn", data.inn),
        ("address", data.address),
        ("phone", data.phone),
        ("email", data.email),
        ("signer_full_name", data.signer_full_name),
    ] {
        match value {
            Patch::Missing => {}
            Patch::Null => {
                qb.push(format!(", {column} = NULL"));
                fields.push(column);
            }
            Patch::Value(v) => {
                qb.push(format!(", {column} = ")).push_bind(v);
                fields.push(column);
            }
        }
    }
    if !fields.is_empty() {
        qb.push(" WHERE id = ").push_bind(id);
        qb.build().execute(&state.db).await?;
    }
    fields.sort();
    audit!(Some(actor.id), "correspondent_updated", "correspondent_id": id, "fields": fields);
    get(state, id).await
}
