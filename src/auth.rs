//! Аутентификация и RBAC через экстракторы axum.
//!
//! Каждый защищенный endpoint проверяет: валидность токена, существование и активность пользователя
//! (роль всегда читается из БД), разрешение роли. Контекстные ограничения — в сервисном слое.
//! Проверка выполняется до разбора тела запроса: 401/403 имеют приоритет над 422.

use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;

use crate::error::AppError;
use crate::models::User;
use crate::security::{Permission, decode_access_token, has_permission};
use crate::services::users::find_user;
use crate::state::AppState;

/// Текущий аутентифицированный и активный пользователь.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let header_value = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
        let (scheme, token) = header_value.split_once(' ').unwrap_or((header_value, ""));
        let token = token.trim();
        if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
            return Err(AppError::unauthorized("Требуется аутентификация (заголовок Authorization: Bearer <token>)"));
        }
        let user_id = decode_access_token(token, &state.settings.jwt_secret).map_err(|_| {
            AppError::unauthorized("Недействительный или просроченный токен").with_code("INVALID_TOKEN")
        })?;
        let user = find_user(&state.db, user_id)
            .await?
            .ok_or_else(|| AppError::unauthorized("Пользователь не найден").with_code("INVALID_TOKEN"))?;
        if !user.is_active {
            return Err(AppError::forbidden("Учетная запись деактивирована").with_code("USER_INACTIVE"));
        }
        Ok(CurrentUser(user))
    }
}

/// Набор разрешений для экстрактора [`Authorized`].
pub trait PermissionSpec: Send + Sync {
    const PERMISSIONS: &'static [Permission];
    /// `true` — достаточно любого из разрешений, `false` — нужны все.
    const ANY_OF: bool = false;
}

/// Пользователь, роль которого имеет нужные разрешения.
pub struct Authorized<P: PermissionSpec> {
    pub user: User,
    _spec: PhantomData<P>,
}

impl<P: PermissionSpec> FromRequestParts<AppState> for Authorized<P> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        let mut checks = P::PERMISSIONS.iter().map(|p| has_permission(user.role, *p));
        let allowed = if P::ANY_OF { checks.any(|ok| ok) } else { checks.all(|ok| ok) };
        if !allowed {
            return Err(AppError::forbidden("Недостаточно прав для выполнения операции"));
        }
        Ok(Self { user, _spec: PhantomData })
    }
}

macro_rules! permission_spec {
    ($($name:ident => [$($perm:ident),+] $(, $any:ident)?;)+) => {
        $(
            pub struct $name;
            impl PermissionSpec for $name {
                const PERMISSIONS: &'static [Permission] = &[$(Permission::$perm),+];
                $(const ANY_OF: bool = permission_spec!(@any $any);)?
            }
        )+
    };
    (@any any_of) => { true };
}

/// Наборы разрешений, используемые обработчиками.
pub mod can {
    use super::{Permission, PermissionSpec};

    permission_spec! {
        CreateUser => [UserCreate];
        ViewUsers => [UserView];
        EditUser => [UserEdit];
        DeactivateUser => [UserDeactivate];
        ListExecutors => [ResolutionCreate, UserView], any_of;
        ManageDictionaries => [DictionaryManage];
        CreateCorrespondent => [CorrespondentCreate];
        EditCorrespondent => [CorrespondentEdit];
        ViewCorrespondents => [CorrespondentView];
        RegisterDocument => [DocumentRegister];
        EditDocument => [DocumentEdit];
        ViewDocuments => [DocumentView];
        DeleteDocument => [DocumentDelete];
        ChangeStatus => [StatusChange, DocumentAnnul], any_of;
        CreateResolution => [ResolutionCreate];
        EditResolution => [ResolutionEdit];
        ViewResolutions => [ResolutionView];
        AttachFile => [FileAttach];
        GenerateReport => [ReportGenerate];
        ExportRegistry => [ExportRegistry];
    }
}
