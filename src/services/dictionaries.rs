//! Справочники: подразделения, должности, типы документов.

use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::audit;
use crate::dto::{DepartmentOut, DocumentTypeOut, PositionOut};
use crate::error::{AppError, AppResult};
use crate::models::{Department, DocumentType, Position, User};
use crate::state::AppState;

#[derive(Debug, Clone, Copy)]
pub enum Dictionary {
    Departments,
    Positions,
    DocumentTypes,
}

impl Dictionary {
    fn table(self) -> &'static str {
        match self {
            Self::Departments => "departments",
            Self::Positions => "positions",
            Self::DocumentTypes => "document_types",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Departments => "Подразделение",
            Self::Positions => "Должность",
            Self::DocumentTypes => "Тип документа",
        }
    }

    fn columns(self) -> &'static str {
        match self {
            Self::Departments => "id, name, created_at, updated_at",
            Self::Positions => "id, name",
            Self::DocumentTypes => "id, name, is_active",
        }
    }
}

async fn exists(state: &AppState, dict: Dictionary, id: Uuid) -> AppResult<bool> {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {} WHERE id = $1)", dict.table());
    Ok(sqlx::query_scalar(&sql).bind(id).fetch_one(&state.db).await?)
}

async fn id_by_name(state: &AppState, dict: Dictionary, name: &str) -> AppResult<Option<Uuid>> {
    let sql = format!("SELECT id FROM {} WHERE name = $1", dict.table());
    Ok(sqlx::query_scalar(&sql).bind(name).fetch_optional(&state.db).await?)
}

fn name_conflict(dict: Dictionary) -> AppError {
    AppError::conflict(format!("{} с таким названием уже существует", dict.label()))
        .with_code("NAME_CONFLICT")
        .with_field("name")
}

async fn fetch<T>(state: &AppState, dict: Dictionary, id: Uuid) -> AppResult<T>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    let sql = format!("SELECT {} FROM {} WHERE id = $1", dict.columns(), dict.table());
    sqlx::query_as::<_, T>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found(format!("{}: запись не найдена", dict.label())))
}

async fn list<T>(state: &AppState, dict: Dictionary) -> AppResult<Vec<T>>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    let sql = format!("SELECT {} FROM {} ORDER BY name", dict.columns(), dict.table());
    Ok(sqlx::query_as::<_, T>(&sql).fetch_all(&state.db).await?)
}

async fn create_row(state: &AppState, actor: &User, dict: Dictionary, name: &str, is_active: Option<bool>) -> AppResult<Uuid> {
    if id_by_name(state, dict, name).await?.is_some() {
        return Err(name_conflict(dict));
    }
    let id = Uuid::new_v4();
    let mut qb = QueryBuilder::<Postgres>::new(format!("INSERT INTO {} (id, name", dict.table()));
    if is_active.is_some() {
        qb.push(", is_active");
    }
    qb.push(") VALUES (").push_bind(id).push(", ").push_bind(name.to_string());
    if let Some(active) = is_active {
        qb.push(", ").push_bind(active);
    }
    qb.push(")");
    qb.build().execute(&state.db).await?;
    audit!(Some(actor.id), "dictionary_created", "dictionary": dict.table(), "item_id": id);
    Ok(id)
}

async fn update_row(
    state: &AppState,
    actor: &User,
    dict: Dictionary,
    id: Uuid,
    name: Option<String>,
    is_active: Option<bool>,
) -> AppResult<()> {
    if !exists(state, dict, id).await? {
        return Err(AppError::not_found(format!("{}: запись не найдена", dict.label())));
    }
    if let Some(name) = &name
        && let Some(other) = id_by_name(state, dict, name).await?
        && other != id
    {
        return Err(name_conflict(dict));
    }
    let mut fields: Vec<&str> = Vec::new();
    let mut qb = QueryBuilder::<Postgres>::new(format!("UPDATE {} SET ", dict.table()));
    let mut set = qb.separated(", ");
    if let Some(name) = name {
        set.push("name = ").push_bind_unseparated(name);
        fields.push("name");
    }
    if let Some(active) = is_active {
        set.push("is_active = ").push_bind_unseparated(active);
        fields.push("is_active");
    }
    if matches!(dict, Dictionary::Departments) && !fields.is_empty() {
        set.push("updated_at = now()");
    }
    if fields.is_empty() {
        return Ok(());
    }
    qb.push(" WHERE id = ").push_bind(id);
    qb.build().execute(&state.db).await?;
    fields.sort();
    audit!(Some(actor.id), "dictionary_updated", "item_id": id, "fields": fields);
    Ok(())
}

// --- подразделения ----------------------------------------------------------------------------------

pub async fn list_departments(state: &AppState) -> AppResult<Vec<DepartmentOut>> {
    Ok(list::<Department>(state, Dictionary::Departments).await?.into_iter().map(Into::into).collect())
}

pub async fn create_department(state: &AppState, actor: &User, name: &str) -> AppResult<DepartmentOut> {
    let id = create_row(state, actor, Dictionary::Departments, name, None).await?;
    Ok(fetch::<Department>(state, Dictionary::Departments, id).await?.into())
}

pub async fn update_department(state: &AppState, actor: &User, id: Uuid, name: Option<String>) -> AppResult<DepartmentOut> {
    update_row(state, actor, Dictionary::Departments, id, name, None).await?;
    Ok(fetch::<Department>(state, Dictionary::Departments, id).await?.into())
}

// --- должности ------------------------------------------------------------------------------------

pub async fn list_positions(state: &AppState) -> AppResult<Vec<PositionOut>> {
    Ok(list::<Position>(state, Dictionary::Positions).await?.into_iter().map(Into::into).collect())
}

pub async fn create_position(state: &AppState, actor: &User, name: &str) -> AppResult<PositionOut> {
    let id = create_row(state, actor, Dictionary::Positions, name, None).await?;
    Ok(fetch::<Position>(state, Dictionary::Positions, id).await?.into())
}

pub async fn update_position(state: &AppState, actor: &User, id: Uuid, name: Option<String>) -> AppResult<PositionOut> {
    update_row(state, actor, Dictionary::Positions, id, name, None).await?;
    Ok(fetch::<Position>(state, Dictionary::Positions, id).await?.into())
}

// --- типы документов ------------------------------------------------------------------------------

pub async fn list_types(state: &AppState) -> AppResult<Vec<DocumentTypeOut>> {
    Ok(list::<DocumentType>(state, Dictionary::DocumentTypes).await?.into_iter().map(Into::into).collect())
}

pub async fn create_type(state: &AppState, actor: &User, name: &str, is_active: bool) -> AppResult<DocumentTypeOut> {
    let id = create_row(state, actor, Dictionary::DocumentTypes, name, Some(is_active)).await?;
    Ok(fetch::<DocumentType>(state, Dictionary::DocumentTypes, id).await?.into())
}

pub async fn update_type(
    state: &AppState,
    actor: &User,
    id: Uuid,
    name: Option<String>,
    is_active: Option<bool>,
) -> AppResult<DocumentTypeOut> {
    update_row(state, actor, Dictionary::DocumentTypes, id, name, is_active).await?;
    Ok(fetch::<DocumentType>(state, Dictionary::DocumentTypes, id).await?.into())
}
