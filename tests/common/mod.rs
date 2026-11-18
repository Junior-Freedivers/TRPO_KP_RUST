//! Инфраструктура интеграционных тестов: реальные PostgreSQL и SeaweedFS (docker compose).
//!
//! Тесты работают с отдельной БД (<имя>_test) и бакетом (TEST_S3_BUCKET, по умолчанию correspondence-test),
//! боевые данные не затрагиваются. Запуск: `docker compose --profile test run --rm tests`.
#![allow(dead_code)]
// Блокировка SERIAL удерживается на протяжении всего теста намеренно: #[tokio::test] выполняется
// в однопоточном runtime, а тесты должны идти строго по одному (общие таблицы очищаются).
#![allow(clippy::await_holding_lock)]

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use bytes::Bytes;
use chrono::{Duration, NaiveDate};
use correspondence::config::normalize_database_url;
use correspondence::security::{create_access_token, hash_password};
use correspondence::{AppState, Settings, build_router, init_state};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::Connection;
use tower::ServiceExt;
use uuid::Uuid;

pub const PDF_BYTES: &[u8] = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n";

pub fn png_bytes() -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend([0u8; 64]);
    data
}

/// логин -> (роль, пароль, ФИО)
pub const USERS: [(&str, &str, &str, &str); 5] = [
    ("admin", "admin", "admin123", "Романов Игорь Викторович"),
    ("clerk", "clerk", "clerk123", "Иванова Анна Сергеевна"),
    ("manager", "manager", "manager123", "Петров Алексей Николаевич"),
    ("executor1", "executor", "executor123", "Сидорова Мария Игоревна"),
    ("executor2", "executor", "executor123", "Козлов Дмитрий Андреевич"),
];

const TABLES: &str = "document_history, attachments, resolutions, incoming_documents, registration_counters, \
                      correspondents, users, document_types, positions, departments";

/// Интеграционные тесты очищают общие таблицы, поэтому выполняются строго по одному.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Настройки тестового окружения на основе .env / переменных окружения.
pub fn test_settings() -> Settings {
    let _ = dotenvy::dotenv();
    let mut settings = Settings::from_env().expect("для интеграционных тестов нужны переменные из .env");
    let url = normalize_database_url(&settings.database_url);
    let (head, name) = url.rsplit_once('/').expect("DATABASE_URL с именем БД");
    let name = name.split('?').next().unwrap();
    if !name.ends_with("_test") {
        settings.database_url = format!("{head}/{name}_test");
    }
    settings.s3_bucket = std::env::var("TEST_S3_BUCKET").unwrap_or_else(|_| "correspondence-test".into());
    settings.max_upload_size_mb = 1;
    settings.reg_number_template = "ВХ-{YYYY}-{NNNNNN}".into();
    settings.deadline_warning_days = 3;
    settings
}

/// Создает тестовую БД, если ее нет.
async fn ensure_database(url: &str) {
    let (head, name) = url.rsplit_once('/').unwrap();
    let mut conn = sqlx::PgConnection::connect(&format!("{head}/postgres")).await.expect("подключение к PostgreSQL");
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    if !exists {
        sqlx::query(&format!("CREATE DATABASE \"{name}\"")).execute(&mut conn).await.unwrap();
    }
}

fn password_hash(password: &str) -> String {
    static HASHES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let map = HASHES.get_or_init(Default::default);
    let mut map = map.lock().unwrap_or_else(|p| p.into_inner());
    map.entry(password.to_string()).or_insert_with(|| hash_password(password)).clone()
}

/// Ответ тестового запроса.
pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("не JSON: {}", self.text()))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> String {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
    }

    /// Проверяет HTTP-код и единый формат ошибки {detail, code, field}.
    pub fn assert_error(&self, status: u16, code: Option<&str>, field: Option<&str>) -> Value {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        let body = self.json();
        assert!(body["detail"].as_str().is_some_and(|d| !d.is_empty()), "{body}");
        assert!(body["code"].is_string(), "{body}");
        if let Some(code) = code {
            assert_eq!(body["code"], code, "{body}");
        }
        if let Some(field) = field {
            assert_eq!(body["field"], field, "{body}");
        }
        body
    }

    pub fn ok(&self, status: u16) -> Value {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        if self.body.is_empty() { Value::Null } else { self.json() }
    }
}

pub async fn send(app: &Router, request: Request<Body>) -> Resp {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Resp { status, headers, body }
}

pub struct Ctx {
    pub app: Router,
    pub state: AppState,
    pub users: HashMap<&'static str, Uuid>,
    pub correspondents: Vec<Uuid>,
    pub doc_types: Vec<Uuid>,
    pub inactive_type: Uuid,
    pub department: Uuid,
    pub today: NaiveDate,
    _guard: MutexGuard<'static, ()>,
}

impl Ctx {
    /// Пустая БД без данных (для проверки seed).
    pub async fn empty() -> Ctx {
        let guard = serial();
        let settings = test_settings();
        ensure_database(&settings.database_url).await;
        let state = init_state(settings).await.expect("подготовка состояния");
        state.storage.ensure_bucket().await.expect("бакет в SeaweedFS");
        sqlx::query(&format!("TRUNCATE {TABLES} RESTART IDENTITY CASCADE")).execute(&state.db).await.unwrap();
        let today = state.settings.business_today();
        Ctx {
            app: build_router(state.clone()),
            state,
            users: HashMap::new(),
            correspondents: vec![],
            doc_types: vec![],
            inactive_type: Uuid::nil(),
            department: Uuid::nil(),
            today,
            _guard: guard,
        }
    }

    /// Чистая БД с базовым набором: подразделение, должность, типы, 5 пользователей, 2 корреспондента.
    pub async fn new() -> Ctx {
        let mut ctx = Ctx::empty().await;
        let db = &ctx.state.db;
        let department = Uuid::new_v4();
        let position = Uuid::new_v4();
        sqlx::query("INSERT INTO departments (id, name) VALUES ($1, 'Канцелярия')").bind(department).execute(db).await.unwrap();
        sqlx::query("INSERT INTO positions (id, name) VALUES ($1, 'Специалист')").bind(position).execute(db).await.unwrap();
        let mut types = Vec::new();
        for (name, active) in [("Письмо", true), ("Претензия", true), ("Устаревший тип", false)] {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO document_types (id, name, is_active) VALUES ($1, $2, $3)")
                .bind(id)
                .bind(name)
                .bind(active)
                .execute(db)
                .await
                .unwrap();
            types.push(id);
        }
        for (login, role, password, full_name) in USERS {
            let id = Uuid::new_v4();
            let hash = tokio::task::spawn_blocking(move || password_hash(password)).await.unwrap();
            sqlx::query(
                "INSERT INTO users (id, full_name, login, email, password_hash, role, department_id, position_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(id)
            .bind(full_name)
            .bind(login)
            .bind(format!("{login}@example.org"))
            .bind(hash)
            .bind(role)
            .bind(department)
            .bind(position)
            .execute(db)
            .await
            .unwrap();
            ctx.users.insert(login, id);
        }
        for (name, inn, signer) in [
            ("ООО «Северный ветер»", "7701000001", "Кузнецов И. И."),
            ("АО «Прогресс-Инжиниринг»", "7702000002", "Орлова Т. А."),
        ] {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO correspondents (id, name, inn, signer_full_name) VALUES ($1, $2, $3, $4)")
                .bind(id)
                .bind(name)
                .bind(inn)
                .bind(signer)
                .execute(db)
                .await
                .unwrap();
            ctx.correspondents.push(id);
        }
        ctx.inactive_type = types[2];
        ctx.doc_types = types[..2].to_vec();
        ctx.department = department;
        ctx
    }

    pub fn token(&self, login: &str) -> String {
        create_access_token(self.users[login], &self.state.settings.jwt_secret, 15, None)
    }

    pub async fn call(&self, method: Method, uri: &str, login: Option<&str>, body: Option<Value>) -> Resp {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(login) = login {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", self.token(login)));
        }
        let request = match body {
            Some(json) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&json).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        send(&self.app, request).await
    }

    pub async fn get(&self, uri: &str, login: Option<&str>) -> Resp {
        self.call(Method::GET, uri, login, None).await
    }

    pub async fn post(&self, uri: &str, login: Option<&str>, body: Value) -> Resp {
        self.call(Method::POST, uri, login, Some(body)).await
    }

    pub async fn patch(&self, uri: &str, login: Option<&str>, body: Value) -> Resp {
        self.call(Method::PATCH, uri, login, Some(body)).await
    }

    pub async fn delete(&self, uri: &str, login: Option<&str>) -> Resp {
        self.call(Method::DELETE, uri, login, None).await
    }

    /// GET с параметрами строки запроса.
    pub async fn get_query(&self, path: &str, login: &str, params: &[(&str, String)]) -> Resp {
        let query = serde_urlencoded::to_string(params).unwrap();
        self.get(&format!("{path}?{query}"), Some(login)).await
    }

    pub async fn sql(&self, statement: &str, binds: &[Value]) {
        let mut query = sqlx::query(statement);
        for value in binds {
            let s = value.as_str().unwrap();
            query = match (Uuid::parse_str(s), s.parse::<NaiveDate>()) {
                (Ok(id), _) => query.bind(id),
                (_, Ok(date)) => query.bind(date),
                _ => query.bind(s.to_string()),
            };
        }
        query.execute(&self.state.db).await.unwrap();
    }

    // --- сценарные помощники (все действия идут через REST API) -------------------------------

    pub fn date(&self, days: i64) -> String {
        (self.today + Duration::days(days)).to_string()
    }

    pub async fn register_with(&self, overrides: Value) -> Value {
        let mut payload = json!({
            "received_date": self.date(0),
            "correspondent_id": self.correspondents[0],
            "addressee": "Директору",
            "summary": "Запрос сведений о ходе исполнения договора поставки",
            "document_type_id": self.doc_types[0],
            "page_count": 3,
            "execution_deadline": self.date(10),
        });
        for (k, v) in overrides.as_object().unwrap() {
            payload[k] = v.clone();
        }
        self.post("/api/v1/incoming", Some("clerk"), payload).await.ok(201)
    }

    pub async fn register(&self) -> Value {
        self.register_with(json!({})).await
    }

    pub async fn set_status(&self, doc_id: &Value, status: &str, login: &str, comment: Option<&str>) -> Resp {
        self.post(&format!("/api/v1/incoming/{}/status", id(doc_id)), Some(login), json!({"status": status, "comment": comment}))
            .await
    }

    pub async fn to_review(&self, doc_id: &Value) {
        self.set_status(doc_id, "на рассмотрении", "clerk", Some("Передано руководителю")).await.ok(200);
    }

    pub async fn resolve(&self, doc_id: &Value, executor: &str) -> Value {
        self.resolve_days(doc_id, executor, 7).await
    }

    pub async fn resolve_days(&self, doc_id: &Value, executor: &str, days: i64) -> Value {
        self.post(
            &format!("/api/v1/incoming/{}/resolutions", id(doc_id)),
            Some("manager"),
            json!({"text": "Подготовить ответ", "assigned_executor_id": self.users[executor], "deadline": self.date(days)}),
        )
        .await
        .ok(201)
    }

    pub async fn execute(&self, doc_id: &Value, executor: &str) -> Resp {
        self.set_status(doc_id, "исполнен", executor, Some("Ответ направлен")).await
    }

    pub async fn upload(
        &self,
        doc_id: &Value,
        login: &str,
        name: &str,
        content: &[u8],
        mime: &str,
        attachment_type: Option<&str>,
    ) -> Resp {
        let boundary = "----correspondence-test-boundary";
        let mut body = Vec::new();
        if let Some(kind) = attachment_type {
            body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"attachment_type\"\r\n\r\n{kind}\r\n").bytes());
        }
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n"
            )
            .bytes(),
        );
        body.extend_from_slice(content);
        body.extend(format!("\r\n--{boundary}--\r\n").bytes());
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/incoming/{}/files", id(doc_id)))
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token(login)))
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .unwrap();
        send(&self.app, request).await
    }

    pub async fn upload_pdf(&self, doc_id: &Value, login: &str) -> Resp {
        self.upload(doc_id, login, "скан.pdf", PDF_BYTES, "application/pdf", None).await
    }

    /// Документ, проведенный через весь жизненный цикл до архива.
    pub async fn archived_document(&self) -> Value {
        let doc = self.register().await;
        self.to_review(&doc["id"]).await;
        self.resolve(&doc["id"], "executor1").await;
        self.execute(&doc["id"], "executor1").await.ok(200);
        self.set_status(&doc["id"], "снят с контроля", "clerk", None).await.ok(200);
        self.set_status(&doc["id"], "в архиве", "clerk", None).await.ok(200)
    }
}

/// Строковое значение id из JSON.
pub fn id(value: &Value) -> String {
    value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())
}
