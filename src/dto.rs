//! Схемы запросов и ответов REST API (контракт совпадает с прежней Python-версией).

use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{
    Attachment, AttachmentType, Correspondent, DeadlineState, Department, DocumentStatus, DocumentType,
    HistoryEntry, Position, ReportType, Resolution, ResolutionStatus, Role, User,
};

// ------------------------------------------------------------------------------------------------
// Вспомогательные типы и проверки
// ------------------------------------------------------------------------------------------------

/// Поле PATCH-запроса: отличает «не передано» от явного `null`.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Patch<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            None => Patch::Null,
            Some(value) => Patch::Value(value),
        })
    }
}

impl<T> Patch<T> {
    pub fn is_set(&self) -> bool {
        !matches!(self, Patch::Missing)
    }

    /// Значение для обязательного поля: явный null — ошибка валидации.
    pub fn required(self, field: &str) -> AppResult<Option<T>> {
        match self {
            Patch::Missing => Ok(None),
            Patch::Null => Err(AppError::invalid_field(field, "Поле не может быть пустым")),
            Patch::Value(v) => Ok(Some(v)),
        }
    }

    /// `null` трактуется как «не изменять» (справочники).
    pub fn ignore_null(self) -> Option<T> {
        match self {
            Patch::Value(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_value(&self) -> Option<&T> {
        match self {
            Patch::Value(v) => Some(v),
            _ => None,
        }
    }
}

pub fn check_len(field: &str, value: &str, min: usize, max: usize) -> AppResult<()> {
    let len = value.chars().count();
    if len < min {
        return Err(AppError::invalid_field(field, format!("Строка должна содержать не менее {min} символов")));
    }
    if len > max {
        return Err(AppError::invalid_field(field, format!("Строка должна содержать не более {max} символов")));
    }
    Ok(())
}

fn check_opt_len(field: &str, value: Option<&String>, max: usize) -> AppResult<()> {
    match value {
        Some(v) => check_len(field, v, 0, max),
        None => Ok(()),
    }
}

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s.]+(\.[^@\s.]+)+$").unwrap());
static INN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{10}|\d{12})$").unwrap());
static LOGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]+$").unwrap());

/// Проверяет email и приводит его к нижнему регистру.
pub fn normalize_email(field: &str, value: &str) -> AppResult<String> {
    let value = value.trim();
    if value.chars().count() > 255 || !EMAIL.is_match(value) {
        return Err(AppError::invalid_field(field, "Некорректный адрес электронной почты"));
    }
    Ok(value.to_lowercase())
}

fn check_inn(value: Option<&String>) -> AppResult<()> {
    match value {
        Some(inn) if !INN.is_match(inn) => Err(AppError::invalid_field("inn", "ИНН: 10 или 12 цифр")),
        _ => Ok(()),
    }
}

fn check_range(field: &str, value: i64, min: i64, max: i64) -> AppResult<()> {
    if value < min || value > max {
        return Err(AppError::invalid_field(field, format!("Значение должно быть в диапазоне {min}..{max}")));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// Общие
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct Page<T: ToSchema> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub pages: i64,
}

impl<T: ToSchema> Page<T> {
    pub fn new(items: Vec<T>, total: i64, page: i64, page_size: i64) -> Self {
        let pages = if page_size > 0 { (total + page_size - 1) / page_size } else { 0 };
        Self { items, total, page, page_size, pages }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// Номер страницы (с 1)
    pub page: Option<i64>,
    /// Размер страницы (1..100)
    pub page_size: Option<i64>,
}

pub fn page_params(page: Option<i64>, page_size: Option<i64>) -> AppResult<(i64, i64)> {
    let page = page.unwrap_or(1);
    let page_size = page_size.unwrap_or(20);
    if page < 1 {
        return Err(AppError::invalid_field("page", "Значение должно быть не меньше 1"));
    }
    check_range("page_size", page_size, 1, 100)?;
    Ok((page, page_size))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NamedBrief {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserBrief {
    pub id: Uuid,
    pub full_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub database: String,
    pub storage: String,
}

// ------------------------------------------------------------------------------------------------
// Авторизация и пользователи
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"login": "clerk", "password": "clerk123"}))]
pub struct LoginRequest {
    pub login: String,
    pub password: String,
}

impl LoginRequest {
    pub fn validate(&self) -> AppResult<()> {
        check_len("login", &self.login, 1, 64)?;
        check_len("password", &self.password, 1, 128)
    }
}

// Хэш пароля в схему намеренно не входит и никогда не возвращается.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserOut {
    pub id: Uuid,
    pub full_name: String,
    pub login: String,
    pub email: String,
    pub role: Role,
    pub department: NamedBrief,
    pub position: Option<NamedBrief>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&User> for UserOut {
    fn from(u: &User) -> Self {
        Self {
            id: u.id,
            full_name: u.full_name.clone(),
            login: u.login.clone(),
            email: u.email.clone(),
            role: u.role,
            department: NamedBrief { id: u.department_id, name: u.department_name.clone() },
            position: match (u.position_id, &u.position_name) {
                (Some(id), Some(name)) => Some(NamedBrief { id, name: name.clone() }),
                _ => None,
            },
            is_active: u.is_active,
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    /// Время жизни токена, секунды
    pub expires_in: i64,
    pub user: UserOut,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    #[serde(flatten)]
    pub user: UserOut,
    /// Разрешения роли (матрица RBAC)
    pub permissions: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutorBrief {
    pub id: Uuid,
    pub full_name: String,
    pub department: NamedBrief,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"full_name": "Сидорова Мария Петровна", "login": "sidorova",
    "email": "sidorova@example.org", "password": "S3cretPass!", "role": "executor",
    "department_id": "00000000-0000-0000-0000-000000000001", "position_id": null}))]
pub struct UserCreate {
    pub full_name: String,
    pub login: String,
    pub email: String,
    pub password: String,
    pub role: Role,
    pub department_id: Uuid,
    #[serde(default)]
    pub position_id: Option<Uuid>,
}

impl UserCreate {
    pub fn validate(&mut self) -> AppResult<()> {
        check_len("full_name", &self.full_name, 2, 255)?;
        check_len("login", &self.login, 3, 64)?;
        if !LOGIN.is_match(&self.login) {
            return Err(AppError::invalid_field("login", "Допустимы латинские буквы, цифры и символы _ . -"));
        }
        self.email = normalize_email("email", &self.email)?;
        check_len("password", &self.password, 8, 128)
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserUpdate {
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub full_name: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub email: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<Role>)]
    pub role: Patch<Role>,
    #[serde(default)]
    #[schema(value_type = Option<Uuid>)]
    pub department_id: Patch<Uuid>,
    #[serde(default)]
    #[schema(value_type = Option<Uuid>)]
    pub position_id: Patch<Uuid>,
}

impl UserUpdate {
    pub fn validate(&mut self) -> AppResult<()> {
        if let Patch::Value(name) = &self.full_name {
            check_len("full_name", name, 2, 255)?;
        }
        if let Patch::Value(email) = &self.email {
            self.email = Patch::Value(normalize_email("email", email)?);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UserListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub role: Option<Role>,
    pub is_active: Option<bool>,
    pub department_id: Option<Uuid>,
    /// ФИО, логин или email
    pub search: Option<String>,
}

// ------------------------------------------------------------------------------------------------
// Справочники
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct DepartmentOut {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Department> for DepartmentOut {
    fn from(d: Department) -> Self {
        Self { id: d.id, name: d.name, created_at: d.created_at, updated_at: d.updated_at }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PositionOut {
    pub id: Uuid,
    pub name: String,
}

impl From<Position> for PositionOut {
    fn from(p: Position) -> Self {
        Self { id: p.id, name: p.name }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DocumentTypeOut {
    pub id: Uuid,
    pub name: String,
    pub is_active: bool,
}

impl From<DocumentType> for DocumentTypeOut {
    fn from(t: DocumentType) -> Self {
        Self { id: t.id, name: t.name, is_active: t.is_active }
    }
}

fn clean_name(name: &str) -> AppResult<String> {
    let name = name.trim().to_string();
    check_len("name", &name, 2, 255)?;
    Ok(name)
}

/// Создание подразделения / должности.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"name": "Юридический отдел"}))]
pub struct NameCreate {
    pub name: String,
}

impl NameCreate {
    pub fn validate(&mut self) -> AppResult<()> {
        self.name = clean_name(&self.name)?;
        Ok(())
    }
}

/// Изменение подразделения / должности (`null` — без изменений).
#[derive(Debug, Deserialize, ToSchema)]
pub struct NameUpdate {
    #[serde(default)]
    pub name: Option<String>,
}

impl NameUpdate {
    pub fn validate(&mut self) -> AppResult<()> {
        if let Some(name) = &self.name {
            self.name = Some(clean_name(name)?);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"name": "Претензия", "is_active": true}))]
pub struct DocumentTypeCreate {
    pub name: String,
    #[serde(default = "default_true")]
    pub is_active: bool,
}

fn default_true() -> bool {
    true
}

impl DocumentTypeCreate {
    pub fn validate(&mut self) -> AppResult<()> {
        self.name = clean_name(&self.name)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DocumentTypeUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

impl DocumentTypeUpdate {
    pub fn validate(&mut self) -> AppResult<()> {
        if let Some(name) = &self.name {
            self.name = Some(clean_name(name)?);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------
// Корреспонденты
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"name": "ООО «Северный ветер»", "inn": "7701234567",
    "address": "г. Москва, ул. Примерная, 1", "phone": "+7 495 000-00-00", "email": "info@example.org",
    "signer_full_name": "Кузнецов И. И."}))]
pub struct CorrespondentCreate {
    pub name: String,
    #[serde(default)]
    pub inn: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub signer_full_name: Option<String>,
}

impl CorrespondentCreate {
    pub fn validate(&mut self) -> AppResult<()> {
        check_len("name", &self.name, 2, 500)?;
        check_inn(self.inn.as_ref())?;
        check_opt_len("address", self.address.as_ref(), 2000)?;
        check_opt_len("phone", self.phone.as_ref(), 64)?;
        check_opt_len("signer_full_name", self.signer_full_name.as_ref(), 255)?;
        if let Some(email) = &self.email {
            self.email = Some(normalize_email("email", email)?);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CorrespondentUpdate {
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub name: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub inn: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub address: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub phone: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub email: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub signer_full_name: Patch<String>,
}

impl CorrespondentUpdate {
    pub fn validate(&mut self) -> AppResult<()> {
        if let Patch::Value(name) = &self.name {
            check_len("name", name, 2, 500)?;
        }
        check_inn(self.inn.as_value())?;
        check_opt_len("address", self.address.as_value(), 2000)?;
        check_opt_len("phone", self.phone.as_value(), 64)?;
        check_opt_len("signer_full_name", self.signer_full_name.as_value(), 255)?;
        if let Patch::Value(email) = &self.email {
            self.email = Patch::Value(normalize_email("email", email)?);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CorrespondentOut {
    pub id: Uuid,
    pub name: String,
    pub inn: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub signer_full_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Correspondent> for CorrespondentOut {
    fn from(c: Correspondent) -> Self {
        Self {
            id: c.id,
            name: c.name,
            inn: c.inn,
            address: c.address,
            phone: c.phone,
            email: c.email,
            signer_full_name: c.signer_full_name,
            created_at: c.created_at,
            updated_at: c.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CorrespondentBrief {
    pub id: Uuid,
    pub name: String,
    pub inn: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CorrespondentListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    /// Наименование или ФИО подписанта
    pub search: Option<String>,
    pub inn: Option<String>,
}

// ------------------------------------------------------------------------------------------------
// Входящие документы
// ------------------------------------------------------------------------------------------------

pub const DOCUMENT_SORT_FIELDS: [&str; 6] =
    ["created_at", "execution_deadline", "received_date", "registration_date", "registration_number", "status"];

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"received_date": "2026-09-28", "correspondent_id": "00000000-0000-0000-0000-000000000003",
    "addressee": "Директору", "summary": "Запрос сведений о ходе исполнения договора",
    "document_type_id": "00000000-0000-0000-0000-000000000004", "page_count": 3,
    "execution_deadline": "2026-10-20"}))]
pub struct DocumentCreate {
    pub received_date: NaiveDate,
    pub correspondent_id: Uuid,
    #[serde(default)]
    pub addressee: Option<String>,
    pub summary: String,
    pub document_type_id: Uuid,
    pub page_count: i64,
    pub execution_deadline: NaiveDate,
}

impl DocumentCreate {
    pub fn validate(&self) -> AppResult<()> {
        check_opt_len("addressee", self.addressee.as_ref(), 255)?;
        check_len("summary", &self.summary, 3, 5000)?;
        check_range("page_count", self.page_count, 1, 100_000)
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DocumentUpdate {
    #[serde(default)]
    #[schema(value_type = Option<NaiveDate>)]
    pub received_date: Patch<NaiveDate>,
    #[serde(default)]
    #[schema(value_type = Option<Uuid>)]
    pub correspondent_id: Patch<Uuid>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub addressee: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub summary: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<Uuid>)]
    pub document_type_id: Patch<Uuid>,
    #[serde(default)]
    #[schema(value_type = Option<i64>)]
    pub page_count: Patch<i64>,
    #[serde(default)]
    #[schema(value_type = Option<NaiveDate>)]
    pub execution_deadline: Patch<NaiveDate>,
}

impl DocumentUpdate {
    pub fn validate(&self) -> AppResult<()> {
        check_opt_len("addressee", self.addressee.as_value(), 255)?;
        if let Patch::Value(summary) = &self.summary {
            check_len("summary", summary, 3, 5000)?;
        }
        if let Patch::Value(pages) = &self.page_count {
            check_range("page_count", *pages, 1, 100_000)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"status": "на рассмотрении", "comment": "Передано руководителю"}))]
pub struct StatusChangeRequest {
    pub status: DocumentStatus,
    #[serde(default)]
    pub comment: Option<String>,
}

impl StatusChangeRequest {
    pub fn validate(&self) -> AppResult<()> {
        check_opt_len("comment", self.comment.as_ref(), 2000)
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DocumentOut {
    pub id: Uuid,
    pub registration_number: String,
    pub received_date: NaiveDate,
    pub registration_date: NaiveDate,
    pub correspondent: CorrespondentBrief,
    pub addressee: Option<String>,
    pub summary: String,
    pub document_type: NamedBrief,
    pub page_count: i32,
    pub execution_deadline: NaiveDate,
    pub status: DocumentStatus,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    // вычисляемые поля контроля сроков
    pub deadline_state: DeadlineState,
    pub is_overdue: bool,
    pub days_left: Option<i64>,
    pub executors: Vec<UserBrief>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DocumentDetail {
    #[serde(flatten)]
    pub document: DocumentOut,
    pub resolutions: Vec<ResolutionOut>,
    pub attachments: Vec<AttachmentOut>,
    pub history: Vec<HistoryOut>,
}

/// Параметры поиска и фильтрации (GET /incoming). Даты — по дате регистрации.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DocumentFilters {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    /// Поля через запятую, «-» перед именем — по убыванию. Допустимые: created_at, execution_deadline,
    /// received_date, registration_date, registration_number, status
    pub sort: Option<String>,
    pub registration_number: Option<String>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub correspondent_id: Option<Uuid>,
    pub executor_id: Option<Uuid>,
    pub status: Option<DocumentStatus>,
    pub document_type_id: Option<Uuid>,
    /// Ключевые слова: содержание, адресат, номер, корреспондент
    pub search: Option<String>,
    /// Только overdue (просрочены) или warning (срок близок)
    pub deadline_state: Option<DeadlineState>,
}

impl DocumentFilters {
    pub fn validate(&self) -> AppResult<(i64, i64, String)> {
        let (page, page_size) = page_params(self.page, self.page_size)?;
        let sort = self.sort.clone().unwrap_or_else(|| "-registration_date".into());
        for token in sort.split(',') {
            if !DOCUMENT_SORT_FIELDS.contains(&token.trim().trim_start_matches('-')) {
                return Err(AppError::invalid_field("sort", format!("недопустимое поле сортировки: {token}")));
            }
        }
        check_opt_len("registration_number", self.registration_number.as_ref(), 64)?;
        check_opt_len("search", self.search.as_ref(), 200)?;
        if matches!(self.deadline_state, Some(DeadlineState::Ok | DeadlineState::Closed)) {
            return Err(AppError::invalid_field("deadline_state", "допустимы только overdue и warning"));
        }
        check_period("date_from", self.date_from, self.date_to)?;
        Ok((page, page_size, sort))
    }

    pub fn has_search_filters(&self) -> bool {
        self.registration_number.is_some()
            || self.date_from.is_some()
            || self.date_to.is_some()
            || self.correspondent_id.is_some()
            || self.executor_id.is_some()
            || self.status.is_some()
            || self.document_type_id.is_some()
            || self.search.is_some()
            || self.deadline_state.is_some()
    }
}

fn check_period(field: &str, from: Option<NaiveDate>, to: Option<NaiveDate>) -> AppResult<()> {
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        let to_field = field.replace("from", "to");
        return Err(AppError::validation(format!("{field} не может быть позже {to_field}")));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// Резолюции, вложения, история
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"text": "Подготовить ответ и согласовать с юристом",
    "assigned_executor_id": "00000000-0000-0000-0000-000000000002", "deadline": "2026-10-15"}))]
pub struct ResolutionCreate {
    pub text: String,
    pub assigned_executor_id: Uuid,
    pub deadline: NaiveDate,
}

impl ResolutionCreate {
    pub fn validate(&self) -> AppResult<()> {
        check_len("text", &self.text, 3, 5000)
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResolutionUpdate {
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub text: Patch<String>,
    #[serde(default)]
    #[schema(value_type = Option<Uuid>)]
    pub assigned_executor_id: Patch<Uuid>,
    #[serde(default)]
    #[schema(value_type = Option<NaiveDate>)]
    pub deadline: Patch<NaiveDate>,
}

impl ResolutionUpdate {
    pub fn validate(&self) -> AppResult<()> {
        if let Patch::Value(text) = &self.text {
            check_len("text", text, 3, 5000)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ResolutionOut {
    pub id: Uuid,
    pub document_id: Uuid,
    pub text: String,
    pub author_id: Uuid,
    pub author: UserBrief,
    pub assigned_executor_id: Uuid,
    pub assigned_executor: UserBrief,
    pub deadline: NaiveDate,
    pub status: ResolutionStatus,
    pub executed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Resolution> for ResolutionOut {
    fn from(r: &Resolution) -> Self {
        Self {
            id: r.id,
            document_id: r.document_id,
            text: r.text.clone(),
            author_id: r.author_id,
            author: UserBrief { id: r.author_id, full_name: r.author_name.clone() },
            assigned_executor_id: r.assigned_executor_id,
            assigned_executor: UserBrief { id: r.assigned_executor_id, full_name: r.executor_name.clone() },
            deadline: r.deadline,
            status: r.status,
            executed_at: r.executed_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AttachmentOut {
    pub id: Uuid,
    pub document_id: Uuid,
    pub original_filename: String,
    pub content_type: String,
    pub size_bytes: i32,
    pub attachment_type: AttachmentType,
    pub uploaded_by: Uuid,
    pub uploader: UserBrief,
    pub created_at: DateTime<Utc>,
}

impl From<&Attachment> for AttachmentOut {
    fn from(a: &Attachment) -> Self {
        Self {
            id: a.id,
            document_id: a.document_id,
            original_filename: a.original_filename.clone(),
            content_type: a.content_type.clone(),
            size_bytes: a.size_bytes,
            attachment_type: a.attachment_type,
            uploaded_by: a.uploaded_by,
            uploader: UserBrief { id: a.uploaded_by, full_name: a.uploader_name.clone() },
            created_at: a.created_at,
        }
    }
}

/// Форма загрузки файла (multipart/form-data) — только для документации OpenAPI.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct UploadForm {
    /// PDF, JPG, PNG или DOCX
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
    pub attachment_type: Option<AttachmentType>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HistoryOut {
    pub id: Uuid,
    pub document_id: Uuid,
    pub user_id: Uuid,
    pub user: UserBrief,
    pub action: String,
    pub old_status: Option<DocumentStatus>,
    pub new_status: Option<DocumentStatus>,
    pub comment: Option<String>,
    #[schema(value_type = Option<Object>)]
    pub metadata: Option<Value>,
    pub created_at: DateTime<Utc>,
}

impl From<HistoryEntry> for HistoryOut {
    fn from(h: HistoryEntry) -> Self {
        Self {
            id: h.id,
            document_id: h.document_id,
            user_id: h.user_id,
            user: UserBrief { id: h.user_id, full_name: h.user_name },
            action: h.action,
            old_status: h.old_status,
            new_status: h.new_status,
            comment: h.comment,
            metadata: h.metadata,
            created_at: h.created_at,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Отчеты и выгрузки
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[schema(example = json!({"report_type": "executors", "period_from": "2026-09-01", "period_to": "2026-09-30"}))]
pub struct ReportRequest {
    pub report_type: ReportType,
    pub period_from: NaiveDate,
    pub period_to: NaiveDate,
    #[serde(default)]
    pub correspondent_id: Option<Uuid>,
    #[serde(default)]
    pub executor_id: Option<Uuid>,
    #[serde(default)]
    pub status: Option<DocumentStatus>,
    #[serde(default)]
    pub document_type_id: Option<Uuid>,
}

impl ReportRequest {
    pub fn validate(&self) -> AppResult<()> {
        check_period("period_from", Some(self.period_from), Some(self.period_to))
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RegistrationLogQuery {
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub correspondent_id: Option<Uuid>,
    pub status: Option<DocumentStatus>,
    pub document_type_id: Option<Uuid>,
}

impl RegistrationLogQuery {
    pub fn validate(&self) -> AppResult<()> {
        check_period("date_from", self.date_from, self.date_to)
    }
}

/// Значение для журнала изменений (даты, UUID, enum -> строки).
pub trait Jsonable {
    fn to_json(&self) -> Value;
}

impl Jsonable for String {
    fn to_json(&self) -> Value {
        json!(self)
    }
}
impl Jsonable for Uuid {
    fn to_json(&self) -> Value {
        json!(self.to_string())
    }
}
impl Jsonable for NaiveDate {
    fn to_json(&self) -> Value {
        json!(self.format("%Y-%m-%d").to_string())
    }
}
impl Jsonable for i64 {
    fn to_json(&self) -> Value {
        json!(self)
    }
}
impl<T: Jsonable> Jsonable for Option<T> {
    fn to_json(&self) -> Value {
        self.as_ref().map_or(Value::Null, Jsonable::to_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_distinguishes_missing_and_null() {
        let u: UserUpdate = serde_json::from_str(r#"{"position_id": null, "full_name": "Иванов И."}"#).unwrap();
        assert_eq!(u.position_id, Patch::Null);
        assert_eq!(u.department_id, Patch::Missing);
        assert_eq!(u.full_name, Patch::Value("Иванов И.".into()));
    }

    #[test]
    fn email_normalization() {
        assert_eq!(normalize_email("email", " Info@Example.ORG ").unwrap(), "info@example.org");
        assert!(normalize_email("email", "no-at-sign").is_err());
        assert!(normalize_email("email", "a@b").is_err());
    }

    #[test]
    fn document_filters_validation() {
        let f = DocumentFilters { sort: Some("-status,registration_number".into()), ..Default::default() };
        assert_eq!(f.validate().unwrap(), (1, 20, "-status,registration_number".into()));
        let bad = DocumentFilters { sort: Some("password".into()), ..Default::default() };
        assert_eq!(bad.validate().unwrap_err().field.as_deref(), Some("sort"));
        let state = DocumentFilters { deadline_state: Some(DeadlineState::Ok), ..Default::default() };
        assert!(state.validate().is_err());
        let period = DocumentFilters {
            date_from: Some("2026-10-02".parse().unwrap()),
            date_to: Some("2026-10-01".parse().unwrap()),
            ..Default::default()
        };
        assert!(period.validate().is_err());
        assert!(!DocumentFilters::default().has_search_filters());
    }

    #[test]
    fn page_count_rounding() {
        let p: Page<UserBrief> = Page::new(vec![], 41, 1, 20);
        assert_eq!(p.pages, 3);
    }
}
