//! Безопасность: матрица разрешений (RBAC), хэширование паролей (argon2), JWT.

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::Role;

// ------------------------------------------------------------------------------------------------
// Матрица разрешений — единый источник истины для RBAC (backend)
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Authenticate,
    UserCreate,
    UserView,
    UserEdit,
    UserDeactivate,
    /// подразделения, должности, типы документов
    DictionaryManage,
    CorrespondentCreate,
    CorrespondentEdit,
    CorrespondentView,
    DocumentRegister,
    DocumentEdit,
    DocumentView,
    DocumentSearch,
    DocumentDelete,
    /// аннулирование (возврат в «зарегистрирован»)
    DocumentAnnul,
    ResolutionCreate,
    ResolutionEdit,
    ResolutionView,
    FileAttach,
    StatusChange,
    ReportGenerate,
    ExportRegistry,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        use Permission::*;
        match self {
            Authenticate => "auth:login",
            UserCreate => "user:create",
            UserView => "user:view",
            UserEdit => "user:edit",
            UserDeactivate => "user:deactivate",
            DictionaryManage => "dictionary:manage",
            CorrespondentCreate => "correspondent:create",
            CorrespondentEdit => "correspondent:edit",
            CorrespondentView => "correspondent:view",
            DocumentRegister => "document:register",
            DocumentEdit => "document:edit",
            DocumentView => "document:view",
            DocumentSearch => "document:search",
            DocumentDelete => "document:delete",
            DocumentAnnul => "document:annul",
            ResolutionCreate => "resolution:create",
            ResolutionEdit => "resolution:edit",
            ResolutionView => "resolution:view",
            FileAttach => "file:attach",
            StatusChange => "document:change_status",
            ReportGenerate => "report:generate",
            ExportRegistry => "export:registry",
        }
    }
}

pub fn role_permissions(role: Role) -> &'static [Permission] {
    use Permission::*;
    match role {
        Role::Clerk => &[
            Authenticate,
            CorrespondentCreate, CorrespondentEdit, CorrespondentView,
            DocumentRegister, DocumentEdit, DocumentView, DocumentSearch,
            ResolutionView, FileAttach, StatusChange,
            ReportGenerate, ExportRegistry,
        ],
        Role::Manager => &[
            Authenticate,
            DocumentView, DocumentSearch,
            ResolutionCreate, ResolutionEdit, ResolutionView,
            ReportGenerate,
        ],
        Role::Executor => &[Authenticate, DocumentView, ResolutionView, StatusChange, FileAttach],
        Role::Admin => &[
            Authenticate,
            UserCreate, UserView, UserEdit, UserDeactivate,
            DictionaryManage,
            DocumentDelete, DocumentAnnul,
            // только чтение карточек: чтобы убедиться, что удаляется именно дубликат/ошибочная запись.
            // Поиск и фильтрация администратору недоступны.
            DocumentView,
        ],
    }
}

pub fn has_permission(role: Role, permission: Permission) -> bool {
    role_permissions(role).contains(&permission)
}

pub fn permissions_of(role: Role) -> Vec<String> {
    let mut list: Vec<String> = role_permissions(role).iter().map(|p| p.as_str().to_string()).collect();
    list.sort();
    list
}

// ------------------------------------------------------------------------------------------------
// Пароли (argon2id). Пароли никогда не хранятся и не логируются в открытом виде.
// ------------------------------------------------------------------------------------------------

fn hasher() -> argon2::Argon2<'static> {
    // те же параметры, что у argon2-cffi по умолчанию (m=64 МиБ, t=3, p=4) — совместимость с прежними хэшами
    let params = argon2::Params::new(65536, 3, 4, Some(32)).expect("корректные параметры argon2");
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

pub fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let salt = SaltString::generate(&mut OsRng);
    hasher().hash_password(password.as_bytes(), &salt).expect("хэширование пароля").to_string()
}

pub fn verify_password(password: &str, password_hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    match PasswordHash::new(password_hash) {
        // параметры берутся из самой строки хэша
        Ok(parsed) => argon2::Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Хэширование/проверка выполняются в пуле блокирующих задач, чтобы не занимать рабочие потоки tokio.
pub async fn hash_password_async(password: String) -> String {
    tokio::task::spawn_blocking(move || hash_password(&password)).await.expect("задача хэширования")
}

pub async fn verify_password_async(password: String, password_hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &password_hash)).await.unwrap_or(false)
}

// ------------------------------------------------------------------------------------------------
// JWT. В токене только user_id (sub); роль всегда берется из БД.
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    iat: i64,
    exp: i64,
}

pub fn create_access_token(user_id: Uuid, secret: &str, expires_minutes: i64, now: Option<DateTime<Utc>>) -> String {
    let issued = now.unwrap_or_else(Utc::now);
    let claims = Claims {
        sub: user_id.to_string(),
        iat: issued.timestamp(),
        exp: (issued + Duration::minutes(expires_minutes)).timestamp(),
    };
    jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("подпись JWT")
}

#[derive(Debug, thiserror::Error)]
#[error("недействительный токен: {0}")]
pub struct InvalidTokenError(String);

pub fn decode_access_token(token: &str, secret: &str) -> Result<Uuid, InvalidTokenError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.set_required_spec_claims(&["exp", "sub"]);
    let data = jsonwebtoken::decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map_err(|e| InvalidTokenError(e.to_string()))?;
    Uuid::parse_str(&data.claims.sub).map_err(|e| InvalidTokenError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-test-secret-test-secret-123";

    #[test]
    fn token_roundtrip() {
        let id = Uuid::new_v4();
        let token = create_access_token(id, SECRET, 5, None);
        assert_eq!(decode_access_token(&token, SECRET).unwrap(), id);
    }

    #[test]
    fn expired_and_foreign_tokens_are_rejected() {
        let id = Uuid::new_v4();
        let old = create_access_token(id, SECRET, 5, Some(Utc::now() - Duration::minutes(10)));
        assert!(decode_access_token(&old, SECRET).is_err());
        let foreign = create_access_token(id, "another-secret-another-secret", 5, None);
        assert!(decode_access_token(&foreign, SECRET).is_err());
        assert!(decode_access_token("garbage", SECRET).is_err());
    }

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("S3cretPass!");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("S3cretPass!", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("x", "not-a-hash"));
    }

    #[test]
    fn rbac_matrix() {
        assert!(has_permission(Role::Clerk, Permission::DocumentRegister));
        assert!(!has_permission(Role::Manager, Permission::DocumentRegister));
        assert!(has_permission(Role::Manager, Permission::ResolutionCreate));
        assert!(!has_permission(Role::Executor, Permission::DocumentSearch));
        assert!(has_permission(Role::Admin, Permission::DocumentView));
        assert!(!has_permission(Role::Admin, Permission::DocumentSearch));
        let perms = permissions_of(Role::Executor);
        let mut sorted = perms.clone();
        sorted.sort();
        assert_eq!(perms, sorted);
    }
}
