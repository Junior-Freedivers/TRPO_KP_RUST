//! Интеграционные тесты REST API с настоящими PostgreSQL и SeaweedFS.
//!
//! По умолчанию пропускаются (`#[ignore]`): им нужна инфраструктура docker compose.
//! Запуск: `docker compose --profile test run --rm tests`
//! (или локально: `cargo test -- --include-ignored --test-threads=1` при запущенных postgres и seaweedfs).

mod common;

use std::collections::HashSet;

use common::{Ctx, PDF_BYTES, id, png_bytes};
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};

const INFRA: &str = "нужны PostgreSQL и SeaweedFS (docker compose --profile test run --rm tests)";

fn ids(page: &Value) -> Vec<String> {
    page["items"].as_array().unwrap().iter().map(|d| id(&d["id"])).collect()
}

fn id_set(page: &Value) -> HashSet<String> {
    ids(page).into_iter().collect()
}

// ------------------------------------------------------------------------------------------------
// Авторизация
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn login_success_returns_token_and_role() {
    let ctx = Ctx::new().await;
    let response = ctx.post("/api/v1/auth/login", None, json!({"login": "clerk", "password": "clerk123"})).await;
    let body = response.ok(200);
    assert_eq!(body["token_type"], "bearer");
    assert!(body["access_token"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(body["user"]["role"], "clerk");
    assert_eq!(body["user"]["login"], "clerk");
    assert!(!response.text().contains("password"));

    let me = common::send(
        &ctx.app,
        axum::http::Request::get("/api/v1/auth/me")
            .header("Authorization", format!("Bearer {}", body["access_token"].as_str().unwrap()))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .ok(200);
    assert!(me["permissions"].as_array().unwrap().contains(&json!("document:register")));
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn wrong_password_and_unknown_login_return_401() {
    let ctx = Ctx::new().await;
    for payload in [json!({"login": "clerk", "password": "wrong-pass"}), json!({"login": "nobody", "password": "clerk123"})] {
        ctx.post("/api/v1/auth/login", None, payload).await.assert_error(401, Some("INVALID_CREDENTIALS"), None);
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn deactivated_user_cannot_login_nor_use_old_token() {
    let ctx = Ctx::new().await;
    let url = format!("/api/v1/users/{}/deactivate", ctx.users["executor2"]);
    let body = ctx.call(axum::http::Method::POST, &url, Some("admin"), None).await.ok(200);
    assert_eq!(body["is_active"], false);

    ctx.post("/api/v1/auth/login", None, json!({"login": "executor2", "password": "executor123"}))
        .await
        .assert_error(403, Some("USER_INACTIVE"), None);
    ctx.get("/api/v1/auth/me", Some("executor2")).await.assert_error(403, Some("USER_INACTIVE"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn role_is_always_loaded_from_db() {
    let ctx = Ctx::new().await;
    ctx.patch(&format!("/api/v1/users/{}", ctx.users["executor1"]), Some("admin"), json!({"role": "clerk"}))
        .await
        .ok(200);
    // токен выпущен для того же пользователя: роль берется из БД, новые права действуют сразу
    assert_eq!(ctx.get("/api/v1/auth/me", Some("executor1")).await.ok(200)["role"], "clerk");
    ctx.get("/api/v1/exports/registration-log", Some("executor1")).await.ok(200);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn missing_or_invalid_token_returns_401() {
    let ctx = Ctx::new().await;
    ctx.get("/api/v1/incoming", None).await.assert_error(401, Some("UNAUTHORIZED"), None);
    let bad = common::send(
        &ctx.app,
        axum::http::Request::get("/api/v1/incoming")
            .header("Authorization", "Bearer not-a-token")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    bad.assert_error(401, Some("INVALID_TOKEN"), None);
}

// ------------------------------------------------------------------------------------------------
// Корреспонденты
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn clerk_creates_edits_and_finds_correspondent() {
    let ctx = Ctx::new().await;
    let payload = json!({"name": "ООО «Альфа-Логистик»", "inn": "7709000009", "address": "г. Ростов-на-Дону",
                         "phone": "+7 863 100-00-09", "email": "Office@Alfa.example", "signer_full_name": "Гусева Л. Р."});
    let created = ctx.post("/api/v1/correspondents", Some("clerk"), payload).await.ok(201);
    assert_eq!(created["email"], "office@alfa.example");
    let cid = id(&created["id"]);

    let updated = ctx.patch(&format!("/api/v1/correspondents/{cid}"), Some("clerk"), json!({"phone": "+7 863 555-55-55"})).await.ok(200);
    assert_eq!(updated["phone"], "+7 863 555-55-55");

    let found = ctx.get_query("/api/v1/correspondents", "clerk", &[("search", "альфа".into())]).await.ok(200);
    assert_eq!(ids(&found), vec![cid]);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn correspondent_validation_and_rbac() {
    let ctx = Ctx::new().await;
    ctx.post("/api/v1/correspondents", Some("clerk"), json!({"name": "ООО «Тест»", "inn": "12AB"}))
        .await
        .assert_error(422, Some("VALIDATION_ERROR"), Some("inn"));
    for login in ["manager", "executor1", "admin"] {
        ctx.post("/api/v1/correspondents", Some(login), json!({"name": "ООО «Тест»"}))
            .await
            .assert_error(403, Some("FORBIDDEN"), None);
    }
}

// ------------------------------------------------------------------------------------------------
// Документы
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn register_document_assigns_number_date_and_history() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let number = doc["registration_number"].as_str().unwrap();
    let re = regex::Regex::new(&format!(r"^ВХ-{}-\d{{6}}$", chrono::Datelike::year(&ctx.today))).unwrap();
    assert!(re.is_match(number), "{number}");
    assert!(number.ends_with("000001"));
    assert_eq!(doc["registration_date"], ctx.date(0));
    assert_eq!(doc["status"], "зарегистрирован");
    assert_eq!(doc["history"][0]["action"], "document_registered");
    assert_eq!(doc["history"][0]["new_status"], "зарегистрирован");
    assert_eq!(id(&doc["history"][0]["user"]["id"]), ctx.users["clerk"].to_string());

    let second = ctx.register().await;
    assert!(second["registration_number"].as_str().unwrap().ends_with("000002"));
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn parallel_registration_produces_unique_numbers() {
    let ctx = Ctx::new().await;
    let futures: Vec<_> = (0..12)
        .map(|i| ctx.register_with(json!({"summary": format!("Параллельная регистрация №{i}")})))
        .collect();
    let docs = futures::future::join_all(futures).await; // конкурентные запросы
    let mut numbers: Vec<u32> = docs
        .iter()
        .map(|d| d["registration_number"].as_str().unwrap()[d["registration_number"].as_str().unwrap().len() - 6..].parse().unwrap())
        .collect();
    numbers.sort();
    assert_eq!(numbers, (1..=12).collect::<Vec<_>>()); // без пропусков и коллизий
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn deadline_before_registration_is_rejected() {
    let ctx = Ctx::new().await;
    let payload = json!({"received_date": ctx.date(0), "correspondent_id": ctx.correspondents[0],
                         "summary": "Проверка сроков", "document_type_id": ctx.doc_types[0], "page_count": 1,
                         "execution_deadline": ctx.date(-1)});
    ctx.post("/api/v1/incoming", Some("clerk"), payload)
        .await
        .assert_error(422, Some("VALIDATION_ERROR"), Some("execution_deadline"));
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn registration_integrity_checks() {
    let ctx = Ctx::new().await;
    let base = json!({"received_date": ctx.date(0), "correspondent_id": ctx.correspondents[0],
                      "summary": "Проверка целостности", "document_type_id": ctx.doc_types[0], "page_count": 2,
                      "execution_deadline": ctx.date(5)});
    let nil = "00000000-0000-0000-0000-000000000000";
    let cases = [
        (json!({"page_count": 0}), "page_count"),
        (json!({"correspondent_id": nil}), "correspondent_id"),
        (json!({"document_type_id": nil}), "document_type_id"),
        (json!({"document_type_id": ctx.inactive_type}), "document_type_id"),
        (json!({"received_date": ctx.date(1)}), "received_date"),
    ];
    for (override_, field) in cases {
        let mut payload = base.clone();
        for (k, v) in override_.as_object().unwrap() {
            payload[k] = v.clone();
        }
        ctx.post("/api/v1/incoming", Some("clerk"), payload).await.assert_error(422, Some("VALIDATION_ERROR"), Some(field));
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn only_clerk_registers_and_edits() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let url = format!("/api/v1/incoming/{}", id(&doc["id"]));
    for login in ["manager", "executor1", "admin"] {
        ctx.patch(&url, Some(login), json!({"page_count": 5})).await.assert_error(403, Some("FORBIDDEN"), None);
    }
    let body = ctx.patch(&url, Some("clerk"), json!({"page_count": 5, "summary": "Уточнено"})).await.ok(200);
    assert_eq!(body["page_count"], 5);
    let change = body["history"].as_array().unwrap().iter().find(|h| h["action"] == "document_updated").unwrap();
    assert_eq!(change["metadata"]["changes"]["page_count"], json!({"old": 3, "new": 5}));
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn status_change_is_written_to_history() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let body = ctx.set_status(&doc["id"], "на рассмотрении", "clerk", Some("Передано руководителю")).await.ok(200);
    assert_eq!(body["status"], "на рассмотрении");

    let history = ctx.get(&format!("/api/v1/incoming/{}/history", id(&doc["id"])), Some("clerk")).await.ok(200);
    let entry = history.as_array().unwrap().last().unwrap();
    assert_eq!(entry["action"], "status_changed");
    assert_eq!(entry["old_status"], "зарегистрирован");
    assert_eq!(entry["new_status"], "на рассмотрении");
    assert_eq!(entry["comment"], "Передано руководителю");
    assert_eq!(id(&entry["user"]["id"]), ctx.users["clerk"].to_string());
    assert!(entry["created_at"].is_string());
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn invalid_transition_returns_409_and_role_check_returns_403() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.set_status(&doc["id"], "исполнен", "clerk", None).await.assert_error(409, Some("INVALID_STATUS_TRANSITION"), Some("status"));
    ctx.set_status(&doc["id"], "в архиве", "clerk", None).await.assert_error(409, Some("INVALID_STATUS_TRANSITION"), None);
    ctx.set_status(&doc["id"], "на рассмотрении", "manager", None).await.assert_error(403, Some("FORBIDDEN"), None);
    ctx.set_status(&doc["id"], "на рассмотрении", "admin", None).await.assert_error(403, Some("FORBIDDEN"), None);
    let history = ctx.get(&format!("/api/v1/incoming/{}/history", id(&doc["id"])), Some("clerk")).await.ok(200);
    assert_eq!(history.as_array().unwrap().len(), 1); // отклоненные попытки не пишутся в историю
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn history_is_read_only() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let url = format!("/api/v1/incoming/{}/history", id(&doc["id"]));
    for method in [axum::http::Method::POST, axum::http::Method::PUT, axum::http::Method::PATCH, axum::http::Method::DELETE] {
        assert_eq!(ctx.call(method, &url, Some("admin"), None).await.status.as_u16(), 405);
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn archived_document_cannot_be_edited() {
    let ctx = Ctx::new().await;
    let doc = ctx.archived_document().await;
    assert_eq!(doc["status"], "в архиве");
    assert!(doc["archived_at"].is_string());
    ctx.patch(&format!("/api/v1/incoming/{}", id(&doc["id"])), Some("clerk"), json!({"summary": "Попытка изменить"}))
        .await
        .assert_error(409, Some("DOCUMENT_ARCHIVED"), None);
    ctx.upload_pdf(&doc["id"], "clerk").await.assert_error(409, Some("DOCUMENT_ARCHIVED"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn admin_annuls_document_back_to_registered() {
    let ctx = Ctx::new().await;
    let doc = ctx.archived_document().await;
    let body = ctx.set_status(&doc["id"], "зарегистрирован", "admin", Some("Ошибочная регистрация")).await.ok(200);
    assert_eq!(body["status"], "зарегистрирован");
    assert!(body["archived_at"].is_null());
    assert_eq!(body["history"].as_array().unwrap().last().unwrap()["action"], "document_annulled");
    ctx.set_status(&doc["id"], "зарегистрирован", "admin", None).await.assert_error(409, None, None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn only_admin_can_delete_document() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let upload = ctx.upload_pdf(&doc["id"], "clerk").await.ok(201);
    let url = format!("/api/v1/incoming/{}", id(&doc["id"]));
    for login in ["clerk", "manager", "executor1"] {
        ctx.delete(&url, Some(login)).await.assert_error(403, None, None);
    }
    assert_eq!(ctx.delete(&url, Some("admin")).await.status.as_u16(), 204);
    ctx.get(&url, Some("clerk")).await.assert_error(404, Some("NOT_FOUND"), None);
    ctx.get(&format!("/api/v1/files/{}/download", id(&upload["id"])), Some("clerk")).await.assert_error(404, None, None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn admin_views_but_cannot_search() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.get(&format!("/api/v1/incoming/{}", id(&doc["id"])), Some("admin")).await.ok(200);
    assert_eq!(ctx.get("/api/v1/incoming", Some("admin")).await.ok(200)["total"], 1);
    ctx.get_query("/api/v1/incoming", "admin", &[("status", "зарегистрирован".into())])
        .await
        .assert_error(403, Some("FORBIDDEN"), None);
}

// ------------------------------------------------------------------------------------------------
// Резолюции и исполнение
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn manager_creates_resolution_and_document_goes_to_execution() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.to_review(&doc["id"]).await;
    let resolution = ctx.resolve(&doc["id"], "executor1").await;
    assert_eq!(id(&resolution["author"]["id"]), ctx.users["manager"].to_string());
    assert_eq!(resolution["status"], "на исполнении");

    let card = ctx.get(&format!("/api/v1/incoming/{}", id(&doc["id"])), Some("clerk")).await.ok(200);
    assert_eq!(card["status"], "на исполнении");
    let executors: Vec<&str> = card["executors"].as_array().unwrap().iter().map(|e| e["full_name"].as_str().unwrap()).collect();
    assert_eq!(executors, vec!["Сидорова Мария Игоревна"]);
    let actions: Vec<&str> = card["history"].as_array().unwrap().iter().map(|h| h["action"].as_str().unwrap()).collect();
    assert_eq!(&actions[actions.len() - 2..], &["resolution_created", "status_changed"]);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn resolution_rules() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let body = json!({"text": "Подготовить ответ", "assigned_executor_id": ctx.users["executor1"], "deadline": ctx.date(3)});
    let url = format!("/api/v1/incoming/{}/resolutions", id(&doc["id"]));
    // документ еще не передан на рассмотрение
    ctx.post(&url, Some("manager"), body.clone()).await.assert_error(409, Some("INVALID_DOCUMENT_STATE"), None);
    ctx.to_review(&doc["id"]).await;
    // резолюцию создает только руководитель
    for login in ["clerk", "executor1", "admin"] {
        ctx.post(&url, Some(login), body.clone()).await.assert_error(403, Some("FORBIDDEN"), None);
    }
    // исполнителем может быть только активный пользователь с ролью «исполнитель»
    let mut wrong = body.clone();
    wrong["assigned_executor_id"] = json!(ctx.users["clerk"]);
    ctx.post(&url, Some("manager"), wrong).await.assert_error(422, None, Some("assigned_executor_id"));
    let mut missing = body;
    missing["assigned_executor_id"] = json!("00000000-0000-0000-0000-000000000000");
    ctx.post(&url, Some("manager"), missing).await.assert_error(422, None, None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn executor_sees_only_own_assignments() {
    let ctx = Ctx::new().await;
    let mine = ctx.register_with(json!({"summary": "Поручение исполнителю 1"})).await;
    let foreign = ctx.register_with(json!({"summary": "Поручение исполнителю 2"})).await;
    for (doc, executor) in [(&mine, "executor1"), (&foreign, "executor2")] {
        ctx.to_review(&doc["id"]).await;
        ctx.resolve(&doc["id"], executor).await;
    }
    let listing = ctx.get("/api/v1/incoming", Some("executor1")).await.ok(200);
    assert_eq!(ids(&listing), vec![id(&mine["id"])]);
    ctx.get(&format!("/api/v1/incoming/{}/resolutions", id(&mine["id"])), Some("executor1")).await.ok(200);
    ctx.get(&format!("/api/v1/incoming/{}", id(&foreign["id"])), Some("executor1")).await.assert_error(403, None, None);
    ctx.get(&format!("/api/v1/incoming/{}/resolutions", id(&foreign["id"])), Some("executor1")).await.assert_error(403, None, None);
    // поиск и фильтрация исполнителю недоступны по матрице прав
    ctx.get_query("/api/v1/incoming", "executor1", &[("search", "Поручение".into())]).await.assert_error(403, None, None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn executor_attaches_report_and_marks_execution() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.to_review(&doc["id"]).await;
    ctx.resolve(&doc["id"], "executor1").await;

    let upload = ctx.upload(&doc["id"], "executor1", "Отчет об исполнении.pdf", PDF_BYTES, "application/pdf", None).await.ok(201);
    assert_eq!(upload["attachment_type"], "execution_report");
    ctx.upload(&doc["id"], "executor1", "скан.pdf", PDF_BYTES, "application/pdf", Some("source_scan"))
        .await
        .assert_error(403, None, None);

    ctx.execute(&doc["id"], "executor2").await.assert_error(403, None, None); // чужой документ
    let body = ctx.execute(&doc["id"], "executor1").await.ok(200);
    assert_eq!(body["status"], "исполнен");
    assert_eq!(body["resolutions"][0]["status"], "исполнена");
    assert!(body["resolutions"][0]["executed_at"].is_string());
    ctx.execute(&doc["id"], "executor1").await.assert_error(409, None, None); // повторная отметка
    let closed = ctx.set_status(&doc["id"], "снят с контроля", "clerk", None).await.ok(200);
    assert_eq!(closed["status"], "снят с контроля");
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn document_executed_only_when_all_executors_done() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.to_review(&doc["id"]).await;
    ctx.resolve(&doc["id"], "executor1").await;
    ctx.resolve(&doc["id"], "executor2").await;

    assert_eq!(ctx.execute(&doc["id"], "executor1").await.ok(200)["status"], "на исполнении");
    let second = ctx.execute(&doc["id"], "executor2").await.ok(200);
    assert_eq!(second["status"], "исполнен");
    let entries: Vec<&Value> = second["history"].as_array().unwrap().iter().filter(|h| h["new_status"] == "исполнен").collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(id(&entries[0]["user"]["id"]), ctx.users["executor2"].to_string());
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn resolution_editable_until_completion() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.to_review(&doc["id"]).await;
    let resolution = ctx.resolve(&doc["id"], "executor1").await;
    let url = format!("/api/v1/resolutions/{}", id(&resolution["id"]));

    let edited = ctx
        .patch(&url, Some("manager"), json!({"text": "Подготовить ответ до пятницы", "assigned_executor_id": ctx.users["executor2"]}))
        .await
        .ok(200);
    assert_eq!(id(&edited["assigned_executor"]["id"]), ctx.users["executor2"].to_string());
    ctx.patch(&url, Some("clerk"), json!({"text": "Попытка"})).await.assert_error(403, None, None);

    ctx.execute(&doc["id"], "executor2").await.ok(200);
    ctx.patch(&url, Some("manager"), json!({"text": "Поздно"})).await.assert_error(409, Some("RESOLUTION_LOCKED"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn full_lifecycle_to_archive() {
    let ctx = Ctx::new().await;
    let archived = ctx.archived_document().await;
    let statuses: Vec<&str> = archived["history"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["new_status"].as_str())
        .collect();
    assert_eq!(statuses, ["зарегистрирован", "на рассмотрении", "на исполнении", "исполнен", "снят с контроля", "в архиве"]);
}

// ------------------------------------------------------------------------------------------------
// Файлы (SeaweedFS)
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn clerk_uploads_scan_to_storage_and_downloads_it() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let attachment = ctx.upload(&doc["id"], "clerk", "Скан письма №15.pdf", PDF_BYTES, "application/pdf", None).await.ok(201);
    assert_eq!(attachment["attachment_type"], "source_scan");
    assert_eq!(attachment["size_bytes"], PDF_BYTES.len());

    let card = ctx.get(&format!("/api/v1/incoming/{}", id(&doc["id"])), Some("clerk")).await.ok(200);
    let attached: Vec<String> = card["attachments"].as_array().unwrap().iter().map(|a| id(&a["id"])).collect();
    assert_eq!(attached, vec![id(&attachment["id"])]);
    assert_eq!(card["history"].as_array().unwrap().last().unwrap()["action"], "file_attached");

    let download = ctx.get(&format!("/api/v1/files/{}/download", id(&attachment["id"])), Some("clerk")).await;
    assert_eq!(download.status.as_u16(), 200);
    assert_eq!(&download.body[..], PDF_BYTES);
    assert_eq!(download.header("content-type"), "application/pdf");
    let disposition = percent_decode_str(&download.header("content-disposition")).decode_utf8_lossy().into_owned();
    assert!(disposition.contains("Скан письма №15.pdf"), "{disposition}");
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn storage_key_is_server_generated() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let upload = ctx.upload(&doc["id"], "clerk", "../../etc/passwd.png", &png_bytes(), "image/png", None).await.ok(201);
    assert_eq!(upload["original_filename"], "passwd.png");
    let key: String = sqlx::query_scalar("SELECT storage_key FROM attachments WHERE id = $1")
        .bind(uuid::Uuid::parse_str(&id(&upload["id"])).unwrap())
        .fetch_one(&ctx.state.db)
        .await
        .unwrap();
    assert!(key.starts_with(&format!("documents/{}/", id(&doc["id"]))) && key.ends_with(".png"), "{key}");
    assert!(!key.contains("passwd") && !key.contains(".."));
    assert!(ctx.state.storage.exists(&key).await.unwrap());
    assert_eq!(&ctx.state.storage.read_all(&key).await.unwrap()[..], &png_bytes()[..]);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn download_forbidden_without_rights() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    ctx.to_review(&doc["id"]).await;
    ctx.resolve(&doc["id"], "executor1").await;
    let attachment = ctx.upload_pdf(&doc["id"], "clerk").await.ok(201);
    let url = format!("/api/v1/files/{}/download", id(&attachment["id"]));

    ctx.get(&url, None).await.assert_error(401, None, None);
    ctx.get(&url, Some("executor2")).await.assert_error(403, Some("FORBIDDEN"), None);
    assert_eq!(ctx.get(&url, Some("executor1")).await.status.as_u16(), 200);
    assert_eq!(ctx.get(&url, Some("manager")).await.status.as_u16(), 200);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn invalid_uploads_are_rejected() {
    let ctx = Ctx::new().await;
    let doc = ctx.register().await;
    let mut big = b"%PDF".to_vec();
    big.extend(vec![b'0'; 1024 * 1024 + 10]);
    let cases: [(&str, &[u8], &str, u16, &str); 4] = [
        ("virus.exe", b"MZ\x90\x00", "application/octet-stream", 422, "UNSUPPORTED_FILE_TYPE"),
        ("fake.pdf", b"MZ\x90\x00", "application/pdf", 422, "FILE_CONTENT_MISMATCH"),
        ("scan.pdf", PDF_BYTES, "image/png", 422, "UNSUPPORTED_FILE_TYPE"),
        ("big.pdf", &big, "application/pdf", 413, "FILE_TOO_LARGE"),
    ];
    for (name, content, mime, status, code) in cases {
        ctx.upload(&doc["id"], "clerk", name, content, mime, None).await.assert_error(status, Some(code), None);
    }
    ctx.upload_pdf(&doc["id"], "manager").await.assert_error(403, None, None); // руководитель файлы не прикрепляет
    let card = ctx.get(&format!("/api/v1/incoming/{}", id(&doc["id"])), Some("clerk")).await.ok(200);
    assert_eq!(card["attachments"], json!([]));
}

// ------------------------------------------------------------------------------------------------
// Поиск и контроль сроков
// ------------------------------------------------------------------------------------------------

async fn dataset(ctx: &Ctx) -> (Value, Value, Value) {
    let a = ctx
        .register_with(json!({"summary": "Претензия по качеству поставленного оборудования",
                               "correspondent_id": ctx.correspondents[0], "document_type_id": ctx.doc_types[1]}))
        .await;
    let b = ctx.register_with(json!({"summary": "Запрос сведений о договоре", "correspondent_id": ctx.correspondents[1]})).await;
    let c = ctx.register_with(json!({"summary": "Уведомление о проверке", "correspondent_id": ctx.correspondents[1]})).await;
    ctx.to_review(&b["id"]).await;
    ctx.resolve(&b["id"], "executor2").await;
    // документ «a» зарегистрирован 20 дней назад
    ctx.sql(
        "UPDATE incoming_documents SET received_date = $1, registration_date = $1 WHERE id = $2",
        &[json!(ctx.date(-20)), a["id"].clone()],
    )
    .await;
    (a, b, c)
}

async fn search(ctx: &Ctx, login: &str, params: &[(&str, String)]) -> Value {
    ctx.get_query("/api/v1/incoming", login, params).await.ok(200)
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn search_by_registration_number() {
    let ctx = Ctx::new().await;
    let (a, b, _) = dataset(&ctx).await;
    let number = b["registration_number"].as_str().unwrap();
    let found = search(&ctx, "clerk", &[("registration_number", number[number.len() - 6..].to_string())]).await;
    assert_eq!(ids(&found), vec![id(&b["id"])]);

    let encoded = percent_encoding::utf8_percent_encode(a["registration_number"].as_str().unwrap(), percent_encoding::NON_ALPHANUMERIC);
    let exact = ctx.get(&format!("/api/v1/incoming/by-registration-number/{encoded}"), Some("manager")).await.ok(200);
    assert_eq!(exact["id"], a["id"]);
    let missing = percent_encoding::utf8_percent_encode("ВХ-1999-000001", percent_encoding::NON_ALPHANUMERIC);
    ctx.get(&format!("/api/v1/incoming/by-registration-number/{missing}"), Some("clerk"))
        .await
        .assert_error(404, Some("NOT_FOUND"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn search_by_correspondent_status_type_and_executor() {
    let ctx = Ctx::new().await;
    let (a, b, c) = dataset(&ctx).await;
    let by_corr = search(&ctx, "clerk", &[("correspondent_id", ctx.correspondents[1].to_string())]).await;
    assert_eq!(id_set(&by_corr), HashSet::from([id(&b["id"]), id(&c["id"])]));
    let by_status = search(&ctx, "manager", &[("status", "на исполнении".into())]).await;
    assert_eq!(ids(&by_status), vec![id(&b["id"])]);
    let by_type = search(&ctx, "clerk", &[("document_type_id", ctx.doc_types[1].to_string())]).await;
    assert_eq!(ids(&by_type), vec![id(&a["id"])]);
    let by_executor = search(&ctx, "clerk", &[("executor_id", ctx.users["executor2"].to_string())]).await;
    assert_eq!(ids(&by_executor), vec![id(&b["id"])]);
    assert_eq!(by_executor["items"][0]["executors"][0]["full_name"], "Козлов Дмитрий Андреевич");
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn filter_by_dates() {
    let ctx = Ctx::new().await;
    let (a, b, c) = dataset(&ctx).await;
    let old = search(&ctx, "clerk", &[("date_to", ctx.date(-10))]).await;
    assert_eq!(ids(&old), vec![id(&a["id"])]);
    let recent = search(&ctx, "clerk", &[("date_from", ctx.date(0)), ("date_to", ctx.date(0))]).await;
    assert_eq!(id_set(&recent), HashSet::from([id(&b["id"]), id(&c["id"])]));
    ctx.get_query("/api/v1/incoming", "clerk", &[("date_from", ctx.date(0)), ("date_to", ctx.date(-1))])
        .await
        .assert_error(422, Some("VALIDATION_ERROR"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn keyword_search_is_case_insensitive_and_covers_correspondent() {
    let ctx = Ctx::new().await;
    let (a, b, c) = dataset(&ctx).await;
    assert_eq!(ids(&search(&ctx, "clerk", &[("search", "ПРЕТЕНЗИЯ".into())]).await), vec![id(&a["id"])]);
    let by_name = search(&ctx, "clerk", &[("search", "прогресс".into())]).await;
    assert_eq!(id_set(&by_name), HashSet::from([id(&b["id"]), id(&c["id"])]));
    assert_eq!(search(&ctx, "clerk", &[("search", "100%_".into())]).await["total"], 0); // спецсимволы LIKE экранируются
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn pagination_and_sorting() {
    let ctx = Ctx::new().await;
    dataset(&ctx).await;
    let page1 = search(&ctx, "clerk", &[("page", "1".into()), ("page_size", "2".into()), ("sort", "registration_number".into())]).await;
    let page2 = search(&ctx, "clerk", &[("page", "2".into()), ("page_size", "2".into()), ("sort", "registration_number".into())]).await;
    assert_eq!((page1["total"].as_i64(), page1["pages"].as_i64()), (Some(3), Some(2)));
    assert_eq!((page1["items"].as_array().unwrap().len(), page2["items"].as_array().unwrap().len()), (2, 1));
    let numbers: Vec<String> = page1["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2["items"].as_array().unwrap())
        .map(|d| d["registration_number"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = numbers.clone();
    sorted.sort();
    assert_eq!(numbers, sorted);
    let desc = search(&ctx, "clerk", &[("sort", "-registration_number".into())]).await;
    let desc_numbers: Vec<String> =
        desc["items"].as_array().unwrap().iter().map(|d| d["registration_number"].as_str().unwrap().to_string()).collect();
    sorted.reverse();
    assert_eq!(desc_numbers, sorted);
    ctx.get_query("/api/v1/incoming", "clerk", &[("sort", "password_hash".into())]).await.assert_error(422, None, None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn deadline_control_overdue_and_warning() {
    let ctx = Ctx::new().await;
    let overdue = ctx.register_with(json!({"summary": "Просроченный документ"})).await;
    let warning = ctx.register_with(json!({"summary": "Срок близок", "execution_deadline": ctx.date(2)})).await;
    let normal = ctx.register_with(json!({"summary": "Срок далеко", "execution_deadline": ctx.date(30)})).await;
    ctx.sql(
        "UPDATE incoming_documents SET received_date = $1, registration_date = $1, execution_deadline = $2 WHERE id = $3",
        &[json!(ctx.date(-15)), json!(ctx.date(-2)), overdue["id"].clone()],
    )
    .await;

    let card = ctx.get(&format!("/api/v1/incoming/{}", id(&overdue["id"])), Some("clerk")).await.ok(200);
    assert_eq!((card["deadline_state"].clone(), card["is_overdue"].clone(), card["days_left"].clone()),
               (json!("overdue"), json!(true), json!(-2)));
    assert_eq!(ids(&search(&ctx, "clerk", &[("deadline_state", "overdue".into())]).await), vec![id(&overdue["id"])]);
    assert_eq!(ids(&search(&ctx, "clerk", &[("deadline_state", "warning".into())]).await), vec![id(&warning["id"])]);
    let listing = search(&ctx, "clerk", &[]).await;
    let normal_state = listing["items"].as_array().unwrap().iter().find(|d| d["id"] == normal["id"]).unwrap()["deadline_state"].clone();
    assert_eq!(normal_state, "ok");
}

// ------------------------------------------------------------------------------------------------
// Отчеты и выгрузки
// ------------------------------------------------------------------------------------------------

async fn prepare_reports(ctx: &Ctx) -> (Value, Value) {
    let done = ctx.register_with(json!({"summary": "Письмо о согласовании проекта"})).await;
    ctx.to_review(&done["id"]).await;
    ctx.resolve(&done["id"], "executor1").await;
    ctx.execute(&done["id"], "executor1").await.ok(200);
    let pending = ctx
        .register_with(json!({"summary": "Запрос; с «кавычками» и \"двойными\"", "correspondent_id": ctx.correspondents[1]}))
        .await;
    (done, pending)
}

/// Разбор CSV (разделитель «;», кавычки по RFC 4180) без внешних зависимостей.
fn parse_csv(raw: &[u8]) -> Vec<Vec<String>> {
    let text = String::from_utf8(raw.to_vec()).unwrap();
    let text = text.trim_start_matches('\u{feff}');
    let mut rows = Vec::new();
    let (mut row, mut cell, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match (ch, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (';', false) => row.push(std::mem::take(&mut cell)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => cell.push(c),
        }
    }
    rows
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn csv_registration_log_with_cyrillic() {
    let ctx = Ctx::new().await;
    let (done, pending) = prepare_reports(&ctx).await;
    let response = ctx.get("/api/v1/exports/registration-log", Some("clerk")).await;
    assert_eq!(response.status.as_u16(), 200, "{}", response.text());
    assert!(response.header("content-type").starts_with("text/csv"));
    assert!(response.header("content-disposition").contains("attachment"));
    assert!(response.body.starts_with(b"\xef\xbb\xbf")); // UTF-8 BOM для Excel

    let rows = parse_csv(&response.body);
    let header = &rows[0];
    assert_eq!(header[0], "Регистрационный номер");
    let col = |name: &str| header.iter().position(|h| h == name).unwrap();
    let find = |doc: &Value| rows[1..].iter().find(|r| r[0] == doc["registration_number"].as_str().unwrap()).unwrap().clone();
    assert_eq!(rows.len(), 3);
    assert_eq!(find(&done)[col("Статус")], "исполнен");
    assert_eq!(find(&done)[col("Исполнители")], "Сидорова Мария Игоревна");
    assert_eq!(find(&pending)[col("Корреспондент")], "АО «Прогресс-Инжиниринг»");
    assert_eq!(find(&pending)[col("Краткое содержание")], pending["summary"].as_str().unwrap());
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn csv_filters_and_rbac() {
    let ctx = Ctx::new().await;
    let (done, _) = prepare_reports(&ctx).await;
    let response = ctx.get_query("/api/v1/exports/registration-log", "clerk", &[("status", "исполнен".into())]).await;
    let rows = parse_csv(&response.body);
    let numbers: Vec<&str> = rows[1..].iter().map(|r| r[0].as_str()).collect();
    assert_eq!(numbers, vec![done["registration_number"].as_str().unwrap()]);
    let future = ctx.get_query("/api/v1/exports/registration-log", "clerk", &[("date_from", ctx.date(1))]).await;
    assert_eq!(parse_csv(&future.body).len(), 1); // только заголовок
    for login in ["manager", "executor1", "admin"] {
        ctx.get("/api/v1/exports/registration-log", Some(login)).await.assert_error(403, None, None);
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn pdf_reports_are_generated() {
    let ctx = Ctx::new().await;
    prepare_reports(&ctx).await;
    for report_type in ["documents", "executors", "deadlines"] {
        let payload = json!({"report_type": report_type, "period_from": ctx.date(-30), "period_to": ctx.date(0)});
        for login in ["clerk", "manager"] {
            let response = ctx.post("/api/v1/reports/generate", Some(login), payload.clone()).await;
            assert_eq!(response.status.as_u16(), 200, "{}", response.text());
            assert_eq!(response.header("content-type"), "application/pdf");
            assert!(response.body.starts_with(b"%PDF") && response.body.len() > 2000);
            assert!(response.header("content-disposition").contains(&format!("report-{report_type}")));
        }
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn pdf_report_with_filters_and_validation() {
    let ctx = Ctx::new().await;
    prepare_reports(&ctx).await;
    let payload = json!({"report_type": "documents", "period_from": ctx.date(0), "period_to": ctx.date(0),
                         "correspondent_id": ctx.correspondents[1], "status": "зарегистрирован",
                         "document_type_id": ctx.doc_types[0]});
    ctx.post("/api/v1/reports/generate", Some("clerk"), payload.clone()).await.ok(200);
    let mut bad = payload.clone();
    bad["period_from"] = json!(ctx.date(1));
    ctx.post("/api/v1/reports/generate", Some("clerk"), bad).await.assert_error(422, None, None);
    ctx.post("/api/v1/reports/generate", Some("clerk"), json!({"report_type": "documents"}))
        .await
        .assert_error(422, None, None); // период обязателен
    for login in ["executor1", "admin"] {
        ctx.post("/api/v1/reports/generate", Some(login), payload.clone()).await.assert_error(403, None, None);
    }
}

// ------------------------------------------------------------------------------------------------
// Пользователи и справочники
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn admin_creates_user_without_exposing_password_hash() {
    let ctx = Ctx::new().await;
    let mut payload = json!({"full_name": "Новикова Ольга Петровна", "login": "novikova", "email": "Novikova@Example.org",
                             "password": "StrongPass1", "role": "executor", "department_id": ctx.department});
    let response = ctx.post("/api/v1/users", Some("admin"), payload.clone()).await;
    let body = response.ok(201);
    assert_eq!(body["email"], "novikova@example.org");
    assert_eq!(body["is_active"], true);
    assert!(!response.text().contains("password"));

    ctx.post("/api/v1/auth/login", None, json!({"login": "novikova", "password": "StrongPass1"})).await.ok(200);
    ctx.post("/api/v1/users", Some("admin"), payload.clone()).await.assert_error(409, Some("LOGIN_CONFLICT"), Some("login"));
    payload["login"] = json!("novikova2");
    ctx.post("/api/v1/users", Some("admin"), payload).await.assert_error(409, Some("EMAIL_CONFLICT"), Some("email"));
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn user_validation_errors() {
    let ctx = Ctx::new().await;
    let payload = json!({"full_name": "Тест", "login": "x y", "email": "not-an-email", "password": "123",
                         "role": "superuser", "department_id": ctx.department});
    ctx.post("/api/v1/users", Some("admin"), payload).await.assert_error(422, Some("VALIDATION_ERROR"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn non_admin_cannot_manage_users() {
    let ctx = Ctx::new().await;
    for login in ["clerk", "manager", "executor1"] {
        ctx.get("/api/v1/users", Some(login)).await.assert_error(403, Some("FORBIDDEN"), None);
        let url = format!("/api/v1/users/{}/deactivate", ctx.users["executor2"]);
        ctx.call(axum::http::Method::POST, &url, Some(login), None).await.assert_error(403, Some("FORBIDDEN"), None);
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn user_list_filter_update_and_no_physical_delete() {
    let ctx = Ctx::new().await;
    let body = ctx.get_query("/api/v1/users", "admin", &[("role", "executor".into()), ("page_size", "1".into())]).await.ok(200);
    assert_eq!((body["total"].as_i64(), body["items"].as_array().unwrap().len(), body["pages"].as_i64()), (Some(2), 1, Some(2)));

    let url = format!("/api/v1/users/{}", ctx.users["executor2"]);
    let updated = ctx.patch(&url, Some("admin"), json!({"full_name": "Козлов Дмитрий Андреевич (ред.)"})).await.ok(200);
    assert!(updated["full_name"].as_str().unwrap().ends_with("(ред.)"));
    assert_eq!(ctx.delete(&url, Some("admin")).await.status.as_u16(), 405);
    let url = format!("/api/v1/users/{}/deactivate", ctx.users["admin"]);
    ctx.call(axum::http::Method::POST, &url, Some("admin"), None).await.assert_error(409, Some("CANNOT_DEACTIVATE_SELF"), None);
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn dictionaries_admin_only_but_types_readable_by_all() {
    let ctx = Ctx::new().await;
    ctx.post("/api/v1/departments", Some("admin"), json!({"name": "Отдел кадров"})).await.ok(201);
    ctx.post("/api/v1/departments", Some("admin"), json!({"name": "Отдел кадров"})).await.assert_error(409, Some("NAME_CONFLICT"), None);
    ctx.post("/api/v1/positions", Some("admin"), json!({"name": "Инспектор"})).await.ok(201);
    ctx.post("/api/v1/document-types", Some("clerk"), json!({"name": "Акт"})).await.assert_error(403, None, None);

    let types = ctx.get("/api/v1/document-types", Some("executor1")).await.ok(200);
    assert_eq!(types.as_array().unwrap().len(), 3);
    ctx.get("/api/v1/departments", Some("clerk")).await.assert_error(403, None, None);
}

// ------------------------------------------------------------------------------------------------
// Инфраструктура и демонстрационные данные
// ------------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn health_checks_database_and_storage() {
    let ctx = Ctx::new().await;
    for url in ["/health", "/api/v1/health"] {
        assert_eq!(ctx.get(url, None).await.ok(200), json!({"status": "ok", "database": "ok", "storage": "ok"}));
    }
}

#[tokio::test]
#[ignore = "нужны PostgreSQL и SeaweedFS"]
async fn seed_creates_consistent_demo_data() {
    let ctx = Ctx::empty().await;
    correspondence::seed::seed(&ctx.state.db, &ctx.state.settings).await.unwrap();
    correspondence::seed::seed(&ctx.state.db, &ctx.state.settings).await.unwrap(); // повторный запуск ничего не дублирует

    let count = |table: &'static str| {
        let db = ctx.state.db.clone();
        async move { sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}")).fetch_one(&db).await.unwrap() }
    };
    assert_eq!((count("users").await, count("departments").await, count("positions").await), (5, 3, 5));
    assert_eq!((count("document_types").await, count("correspondents").await, count("incoming_documents").await), (8, 10, 20));
    assert!(count("resolutions").await > 0);

    let docs: Vec<(uuid::Uuid, String, String)> =
        sqlx::query_as("SELECT id, status, registration_number FROM incoming_documents").fetch_all(&ctx.state.db).await.unwrap();
    let statuses: HashSet<&str> = docs.iter().map(|d| d.1.as_str()).collect();
    assert_eq!(statuses.len(), 6); // все статусы представлены
    assert_eq!(docs.iter().map(|d| &d.2).collect::<HashSet<_>>().len(), 20);
    // история не «фальшивая»: последний зафиксированный статус совпадает с текущим статусом документа
    let history: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT document_id, new_status FROM document_history WHERE new_status IS NOT NULL ORDER BY created_at, seq",
    )
    .fetch_all(&ctx.state.db)
    .await
    .unwrap();
    let mut last = std::collections::HashMap::new();
    for (doc, status) in history {
        last.insert(doc, status);
    }
    for (doc, status, _) in &docs {
        assert_eq!(&last[doc], status);
    }

    for (login, password) in [("admin", "admin123"), ("clerk", "clerk123"), ("manager", "manager123"),
                              ("executor1", "executor123"), ("executor2", "executor123")] {
        ctx.post("/api/v1/auth/login", None, json!({"login": login, "password": password})).await.ok(200);
    }
    let _ = INFRA;
}
