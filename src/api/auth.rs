use axum::extract::State;

use crate::auth::CurrentUser;
use crate::dto::{LoginRequest, MeResponse, TokenResponse, UserOut};
use crate::error::{AppResult, ErrorResponse};
use crate::extract::Json;
use crate::security::permissions_of;
use crate::services::users;
use crate::state::AppState;

/// Вход по логину и паролю
///
/// Возвращает JWT (передается в заголовке `Authorization: Bearer <token>`), данные пользователя и роль.
/// Logout — удаление токена на клиенте (токен stateless).
#[utoipa::path(post, path = "/api/v1/auth/login", tag = "auth", request_body = LoginRequest,
    responses(
        (status = 200, body = TokenResponse),
        (status = 401, body = ErrorResponse, description = "Неверный логин или пароль (INVALID_CREDENTIALS)"),
        (status = 403, body = ErrorResponse, description = "Учетная запись деактивирована (USER_INACTIVE)"),
        (status = 422, body = ErrorResponse, description = "Ошибка валидации входных данных"),
    ))]
pub async fn login(State(state): State<AppState>, Json(data): Json<LoginRequest>) -> AppResult<axum::Json<TokenResponse>> {
    data.validate()?;
    Ok(axum::Json(users::login(&state, data).await?))
}

/// Текущий пользователь и его разрешения
#[utoipa::path(get, path = "/api/v1/auth/me", tag = "auth", security(("bearer" = [])),
    responses(
        (status = 200, body = MeResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
    ))]
pub async fn me(CurrentUser(user): CurrentUser) -> axum::Json<MeResponse> {
    axum::Json(MeResponse { user: UserOut::from(&user), permissions: permissions_of(user.role) })
}
