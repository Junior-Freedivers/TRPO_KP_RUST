//! Пользователи: регистрация, редактирование, деактивация (admin) и вход.

use std::sync::OnceLock;

use sqlx::{PgExecutor, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::audit;
use crate::dto::{
    ExecutorBrief, LoginRequest, NamedBrief, Page, TokenResponse, UserCreate, UserListQuery, UserOut, UserUpdate,
    page_params,
};
use crate::error::{AppError, AppResult};
use crate::models::{Role, USER_SELECT, User};
use crate::security::{create_access_token, hash_password_async, verify_password_async};
use crate::services::like_pattern;
use crate::state::AppState;

pub async fn find_user<'e>(db: impl PgExecutor<'e>, user_id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(&format!("{USER_SELECT} WHERE u.id = $1")).bind(user_id).fetch_optional(db).await
}

async fn find_by<'e>(db: impl PgExecutor<'e>, column: &str, value: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(&format!("{USER_SELECT} WHERE u.{column} = $1")).bind(value).fetch_optional(db).await
}

async fn fresh(state: &AppState, user_id: Uuid) -> AppResult<UserOut> {
    let user = find_user(&state.db, user_id).await?.ok_or_else(|| AppError::not_found("Пользователь не найден"))?;
    Ok(UserOut::from(&user))
}

async fn check_refs(state: &AppState, department_id: Option<Uuid>, position_id: Option<Uuid>) -> AppResult<()> {
    if let Some(id) = department_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM departments WHERE id = $1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
        if !exists {
            return Err(AppError::validation("Подразделение не найдено").with_field("department_id"));
        }
    }
    if let Some(id) = position_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM positions WHERE id = $1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
        if !exists {
            return Err(AppError::validation("Должность не найдена").with_field("position_id"));
        }
    }
    Ok(())
}

// --- вход ---------------------------------------------------------------------------------------

fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| crate::security::hash_password("dummy-password-for-timing"))
}

pub async fn login(state: &AppState, data: LoginRequest) -> AppResult<TokenResponse> {
    let login = data.login.trim();
    let Some(user) = find_by(&state.db, "login", login).await? else {
        // выравниваем время ответа для несуществующего логина
        let hash = tokio::task::spawn_blocking(|| dummy_hash().to_string()).await.unwrap_or_default();
        verify_password_async(data.password, hash).await;
        audit!(None, "login_failed", "login": data.login);
        return Err(AppError::unauthorized("Неверный логин или пароль").with_code("INVALID_CREDENTIALS"));
    };
    if !verify_password_async(data.password, user.password_hash.clone()).await {
        audit!(Some(user.id), "login_failed", "login": data.login);
        return Err(AppError::unauthorized("Неверный логин или пароль").with_code("INVALID_CREDENTIALS"));
    }
    if !user.is_active {
        audit!(Some(user.id), "login_denied_inactive", "login": data.login);
        return Err(AppError::forbidden("Учетная запись деактивирована").with_code("USER_INACTIVE"));
    }
    let s = &state.settings;
    let token = create_access_token(user.id, &s.jwt_secret, s.access_token_expire_minutes, None);
    audit!(Some(user.id), "login", "login": user.login);
    Ok(TokenResponse {
        access_token: token,
        token_type: "bearer".into(),
        expires_in: s.access_token_expire_minutes * 60,
        user: UserOut::from(&user),
    })
}

// --- управление пользователями --------------------------------------------------------------------

pub async fn create(state: &AppState, actor: &User, data: UserCreate) -> AppResult<UserOut> {
    if find_by(&state.db, "login", &data.login).await?.is_some() {
        return Err(AppError::conflict("Пользователь с таким логином уже существует")
            .with_code("LOGIN_CONFLICT")
            .with_field("login"));
    }
    if find_by(&state.db, "email", &data.email).await?.is_some() {
        return Err(AppError::conflict("Пользователь с таким email уже существует")
            .with_code("EMAIL_CONFLICT")
            .with_field("email"));
    }
    check_refs(state, Some(data.department_id), data.position_id).await?;

    let id = Uuid::new_v4();
    let password_hash = hash_password_async(data.password).await;
    sqlx::query(
        "INSERT INTO users (id, full_name, login, email, password_hash, role, department_id, position_id, is_active) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true)",
    )
    .bind(id)
    .bind(data.full_name.trim())
    .bind(&data.login)
    .bind(&data.email)
    .bind(password_hash)
    .bind(data.role)
    .bind(data.department_id)
    .bind(data.position_id)
    .execute(&state.db)
    .await?;
    audit!(Some(actor.id), "user_created", "user_id": id, "login": data.login, "role": data.role);
    fresh(state, id).await
}

pub async fn get(state: &AppState, user_id: Uuid) -> AppResult<UserOut> {
    fresh(state, user_id).await
}

pub async fn list_page(state: &AppState, q: UserListQuery) -> AppResult<Page<UserOut>> {
    let (page, page_size) = page_params(q.page, q.page_size)?;
    if q.search.as_ref().is_some_and(|s| s.chars().count() > 200) {
        return Err(AppError::invalid_field("search", "Строка должна содержать не более 200 символов"));
    }
    let filters = |qb: &mut QueryBuilder<'_, Postgres>| {
        qb.push(" WHERE true");
        if let Some(role) = q.role {
            qb.push(" AND u.role = ").push_bind(role);
        }
        if let Some(active) = q.is_active {
            qb.push(" AND u.is_active = ").push_bind(active);
        }
        if let Some(dep) = q.department_id {
            qb.push(" AND u.department_id = ").push_bind(dep);
        }
        if let Some(search) = q.search.as_deref().filter(|s| !s.is_empty()) {
            let pattern = like_pattern(search);
            qb.push(" AND (u.full_name ILIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\'");
            qb.push(" OR u.login ILIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\'");
            qb.push(" OR u.email ILIKE ").push_bind(pattern).push(" ESCAPE '\\')");
        }
    };
    let mut count = QueryBuilder::new("SELECT count(*) FROM users u");
    filters(&mut count);
    let total: i64 = count.build_query_scalar().fetch_one(&state.db).await?;

    let mut select = QueryBuilder::new(USER_SELECT);
    filters(&mut select);
    select.push(" ORDER BY u.full_name, u.id LIMIT ").push_bind(page_size).push(" OFFSET ").push_bind((page - 1) * page_size);
    let users: Vec<User> = select.build_query_as().fetch_all(&state.db).await?;
    Ok(Page::new(users.iter().map(UserOut::from).collect(), total, page, page_size))
}

pub async fn list_executors(state: &AppState) -> AppResult<Vec<ExecutorBrief>> {
    let users: Vec<User> =
        sqlx::query_as(&format!("{USER_SELECT} WHERE u.role = 'executor' AND u.is_active ORDER BY u.full_name, u.id"))
            .fetch_all(&state.db)
            .await?;
    Ok(users
        .into_iter()
        .map(|u| ExecutorBrief {
            id: u.id,
            full_name: u.full_name,
            department: NamedBrief { id: u.department_id, name: u.department_name },
        })
        .collect())
}

pub async fn update(state: &AppState, actor: &User, user_id: Uuid, data: UserUpdate) -> AppResult<UserOut> {
    let user = find_user(&state.db, user_id).await?.ok_or_else(|| AppError::not_found("Пользователь не найден"))?;
    let position_set = data.position_id.is_set();
    let position_id = data.position_id.as_value().copied();
    let full_name = data.full_name.required("full_name")?;
    let email = data.email.required("email")?;
    let role = data.role.required("role")?;
    let department_id = data.department_id.required("department_id")?;

    if let Some(email) = &email
        && *email != user.email
        && let Some(other) = find_by(&state.db, "email", email).await?
        && other.id != user.id
    {
        return Err(AppError::conflict("Пользователь с таким email уже существует")
            .with_code("EMAIL_CONFLICT")
            .with_field("email"));
    }
    if let Some(role) = role
        && user.id == actor.id
        && role != Role::Admin
    {
        return Err(AppError::conflict("Нельзя лишить себя роли администратора")
            .with_code("CANNOT_CHANGE_OWN_ROLE")
            .with_field("role"));
    }
    check_refs(state, department_id, position_id).await?;

    let mut fields: Vec<&str> = Vec::new();
    let mut qb = QueryBuilder::<Postgres>::new("UPDATE users SET updated_at = now()");
    if let Some(name) = &full_name {
        qb.push(", full_name = ").push_bind(name.trim().to_string());
        fields.push("full_name");
    }
    if let Some(email) = &email {
        qb.push(", email = ").push_bind(email.clone());
        fields.push("email");
    }
    if let Some(role) = role {
        qb.push(", role = ").push_bind(role);
        fields.push("role");
    }
    if let Some(dep) = department_id {
        qb.push(", department_id = ").push_bind(dep);
        fields.push("department_id");
    }
    if position_set {
        qb.push(", position_id = ").push_bind(position_id);
        fields.push("position_id");
    }
    if !fields.is_empty() {
        qb.push(" WHERE id = ").push_bind(user.id);
        qb.build().execute(&state.db).await?;
        fields.sort();
        audit!(Some(actor.id), "user_updated", "user_id": user.id, "fields": fields);
    }
    fresh(state, user.id).await
}

pub async fn deactivate(state: &AppState, actor: &User, user_id: Uuid) -> AppResult<UserOut> {
    let user = find_user(&state.db, user_id).await?.ok_or_else(|| AppError::not_found("Пользователь не найден"))?;
    if user.id == actor.id {
        return Err(AppError::conflict("Нельзя деактивировать собственную учетную запись")
            .with_code("CANNOT_DEACTIVATE_SELF"));
    }
    if user.is_active {
        sqlx::query("UPDATE users SET is_active = false, updated_at = now() WHERE id = $1")
            .bind(user.id)
            .execute(&state.db)
            .await?;
        audit!(Some(actor.id), "user_deactivated", "user_id": user.id, "login": user.login);
    }
    fresh(state, user.id).await
}
