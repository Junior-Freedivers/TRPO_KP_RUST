//! Заполнение БД демонстрационными данными (все персональные данные вымышленные).
//!
//! Запуск: `correspondence seed` (в контейнере: `docker compose exec backend correspondence seed`).
//! Повторный запуск безопасен: если пользователи уже есть, ничего не меняется.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::config::Settings;
use crate::domain::reg_number::{counter_key, format_registration_number};
use crate::models::{DocumentStatus as S, ResolutionStatus, Role};
use crate::security::hash_password;
use crate::services::documents::next_sequence;

const DEPARTMENTS: [&str; 3] = ["Канцелярия", "Юридический отдел", "Финансово-экономический отдел"];
const POSITIONS: [&str; 5] = ["Начальник канцелярии", "Делопроизводитель", "Директор", "Главный юрист", "Ведущий специалист"];
const DOCUMENT_TYPES: [&str; 8] =
    ["Письмо", "Запрос", "Уведомление", "Распоряжение", "Договор", "Претензия", "Обращение граждан", "Другое"];

/// логин, пароль, роль, ФИО, подразделение, должность
const USERS: [(&str, &str, Role, &str, usize, usize); 5] = [
    ("admin", "admin123", Role::Admin, "Романов Игорь Викторович", 0, 4),
    ("clerk", "clerk123", Role::Clerk, "Иванова Анна Сергеевна", 0, 1),
    ("manager", "manager123", Role::Manager, "Петров Алексей Николаевич", 0, 2),
    ("executor1", "executor123", Role::Executor, "Сидорова Мария Игоревна", 1, 3),
    ("executor2", "executor123", Role::Executor, "Козлов Дмитрий Андреевич", 2, 4),
];

const CORRESPONDENTS: [(&str, Option<&str>, &str, &str, &str); 10] = [
    ("ООО «Северный ветер»", Some("7701000001"), "г. Москва, ул. Березовая, д. 1", "+7 495 100-00-01", "Кузнецов И. И."),
    ("АО «Прогресс-Инжиниринг»", Some("7702000002"), "г. Санкт-Петербург, Невский пр., д. 10", "+7 812 100-00-02", "Орлова Т. А."),
    ("ИП Смирнов Олег Павлович", Some("770300000003"), "г. Казань, ул. Кремлевская, д. 5", "+7 843 100-00-03", "Смирнов О. П."),
    ("Управление образования", Some("7704000004"), "г. Тула, пл. Ленина, д. 3", "+7 487 100-00-04", "Белова Н. В."),
    ("ООО «Ромашка-Сервис»", Some("7705000005"), "г. Екатеринбург, ул. Мира, д. 22", "+7 343 100-00-05", "Захаров П. Л."),
    ("ЗАО «ТехноСнаб»", Some("7706000006"), "г. Самара, ул. Заводская, д. 7", "+7 846 100-00-06", "Волков А. Е."),
    ("Комитет по строительству", Some("7707000007"), "г. Пермь, ул. Строителей, д. 12", "+7 342 100-00-07", "Мельникова Е. С."),
    ("НКО «Открытый диалог»", Some("7708000008"), "г. Омск, ул. Партизанская, д. 4", "+7 381 100-00-08", "Тихонов В. Д."),
    ("ООО «Альфа-Логистик»", Some("7709000009"), "г. Ростов-на-Дону, ул. Портовая, д. 9", "+7 863 100-00-09", "Гусева Л. Р."),
    ("Гражданин Никитин С. А.", None, "г. Воронеж, ул. Садовая, д. 15, кв. 8", "+7 473 100-00-10", "Никитин С. А."),
];

const SUMMARIES: [&str; 20] = [
    "Уведомление о расторжении договора поставки", "Запрос сведений о ходе исполнения договора",
    "Претензия по качеству поставленного оборудования", "Обращение по вопросу благоустройства территории",
    "Приглашение на совещание по вопросам взаимодействия", "Письмо о продлении срока действия договора",
    "Запрос копий учредительных документов", "Уведомление об изменении банковских реквизитов",
    "Предложение о сотрудничестве в сфере логистики", "Претензия о нарушении сроков оказания услуг",
    "Обращение гражданина о рассмотрении жалобы", "Распоряжение о предоставлении отчетности",
    "Запрос информации об исполнении поручения", "Письмо о согласовании проектной документации",
    "Уведомление о проведении проверки", "Договор на оказание консультационных услуг (проект)",
    "Обращение по вопросу выдачи справки", "Запрос заключения по правовым вопросам закупки",
    "Письмо о приостановке платежей", "Предложение по актуализации регламента взаимодействия",
];

/// порядок статусов по «зрелости» документа
const ORDER: [S; 6] = [S::Registered, S::UnderReview, S::InExecution, S::Executed, S::RemovedFromControl, S::Archived];

fn status_for(i: usize) -> S {
    match i {
        0..=1 => S::Archived,
        2..=3 => S::RemovedFromControl,
        4..=6 => S::Executed,
        7..=12 => S::InExecution,
        13..=15 => S::UnderReview,
        _ => S::Registered,
    }
}

struct Event<'a> {
    action: &'a str,
    user: Uuid,
    at: DateTime<Utc>,
    old: Option<S>,
    new: Option<S>,
    comment: Option<&'a str>,
    meta: Option<Value>,
}

async fn log(conn: &mut PgConnection, document_id: Uuid, e: Event<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO document_history (id, document_id, user_id, action, old_status, new_status, comment, metadata, \
         created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(e.user)
    .bind(e.action)
    .bind(e.old)
    .bind(e.new)
    .bind(e.comment)
    .bind(e.meta)
    .bind(e.at)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn seed(pool: &sqlx::PgPool, settings: &Settings) -> anyhow::Result<()> {
    let today: NaiveDate = settings.business_today();
    let mut tx = pool.begin().await?;
    let users_count: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&mut *tx).await?;
    if users_count > 0 {
        println!("БД уже содержит данные — seed пропущен.");
        return Ok(());
    }

    let mut departments = Vec::new();
    for name in DEPARTMENTS {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO departments (id, name) VALUES ($1, $2)").bind(id).bind(name).execute(&mut *tx).await?;
        departments.push(id);
    }
    let mut positions = Vec::new();
    for name in POSITIONS {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO positions (id, name) VALUES ($1, $2)").bind(id).bind(name).execute(&mut *tx).await?;
        positions.push(id);
    }
    let mut types = Vec::new();
    for name in DOCUMENT_TYPES {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO document_types (id, name, is_active) VALUES ($1, $2, true)")
            .bind(id)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        types.push(id);
    }

    let mut users: Vec<(Uuid, &str)> = Vec::new();
    for (login, password, role, full_name, dep, pos) in USERS {
        let id = Uuid::new_v4();
        let hash = {
            let password = password.to_string();
            tokio::task::spawn_blocking(move || hash_password(&password)).await?
        };
        sqlx::query(
            "INSERT INTO users (id, full_name, login, email, password_hash, role, department_id, position_id, is_active) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true)",
        )
        .bind(id)
        .bind(full_name)
        .bind(login)
        .bind(format!("{login}@example.org"))
        .bind(hash)
        .bind(role)
        .bind(departments[dep])
        .bind(positions[pos])
        .execute(&mut *tx)
        .await?;
        users.push((id, full_name));
    }
    let mut correspondents = Vec::new();
    for (idx, (name, inn, address, phone, signer)) in CORRESPONDENTS.into_iter().enumerate() {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO correspondents (id, name, inn, address, phone, email, signer_full_name) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(name)
        .bind(inn)
        .bind(address)
        .bind(phone)
        .bind(format!("office{}@example.org", idx + 1))
        .bind(signer)
        .execute(&mut *tx)
        .await?;
        correspondents.push(id);
    }

    let (clerk, manager) = (users[1].0, users[2].0);
    let executors = [users[3], users[4]];
    let total = SUMMARIES.len();
    let template = &settings.reg_number_template;

    for (i, summary) in SUMMARIES.into_iter().enumerate() {
        let status = status_for(i);
        let reg_date = today - Duration::days(2 * (total - 1 - i) as i64 + 1);
        let base = reg_date.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap()).and_utc();
        let rank = ORDER.iter().position(|s| *s == status).unwrap();

        let deadline = match i {
            7 => today - Duration::days(3), // просрочен
            8 => today - Duration::days(1), // просрочен
            9 => today + Duration::days(2), // предупреждение о сроке
            _ if matches!(status, S::Archived | S::RemovedFromControl | S::Executed) => reg_date + Duration::days(14),
            _ => today + Duration::days(10 + i as i64),
        };
        let executed_at = base + Duration::days(5 + (i % 4) as i64);
        let archived_at = (status == S::Archived).then(|| executed_at + Duration::days(2));

        let year = chrono::Datelike::year(&reg_date);
        let sequence = next_sequence(&mut tx, counter_key(template, year)).await?;
        let number = format_registration_number(template, year, sequence);
        let doc_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO incoming_documents (id, registration_number, received_date, registration_date, \
             correspondent_id, addressee, summary, document_type_id, page_count, execution_deadline, status, \
             created_by, archived_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14)",
        )
        .bind(doc_id)
        .bind(&number)
        .bind(reg_date - Duration::days((i % 3) as i64))
        .bind(reg_date)
        .bind(correspondents[i % correspondents.len()])
        .bind(if i % 2 == 0 { "Директору" } else { "Главному юристу" })
        .bind(summary)
        .bind(types[i % types.len()])
        .bind(1 + ((i * 3) % 12) as i32)
        .bind(deadline)
        .bind(status)
        .bind(clerk)
        .bind(archived_at)
        .bind(base)
        .execute(&mut *tx)
        .await?;

        let ev = |action, user, at, old, new, comment, meta| Event { action, user, at, old, new, comment, meta };
        log(&mut tx, doc_id, ev("document_registered", clerk, base, None, Some(S::Registered), None,
                                Some(json!({"registration_number": number})))).await?;
        if rank >= 1 {
            log(&mut tx, doc_id, ev("status_changed", clerk, base + Duration::hours(2), Some(S::Registered),
                                    Some(S::UnderReview), Some("Передано руководителю"), None)).await?;
        }
        if rank >= 2 {
            // у части документов две резолюции
            let assigned: Vec<(Uuid, &str)> = if i == 8 || i == 11 { executors.to_vec() } else { vec![executors[i % 2]] };
            let created_at = base + Duration::days(1);
            let mut resolutions = Vec::new();
            for (k, (executor_id, executor_name)) in assigned.iter().enumerate() {
                let done = rank >= 3 || (i == 8 && k == 0); // у документа №8 исполнена только первая резолюция
                let res_id = Uuid::new_v4();
                let res_deadline = if rank >= 3 { deadline.min(reg_date + Duration::days(14)) } else { deadline };
                sqlx::query(
                    "INSERT INTO resolutions (id, document_id, text, author_id, assigned_executor_id, deadline, \
                     status, executed_at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)",
                )
                .bind(res_id)
                .bind(doc_id)
                .bind(format!("Рассмотреть и подготовить ответ. Исполнитель: {executor_name}"))
                .bind(manager)
                .bind(executor_id)
                .bind(res_deadline)
                .bind(if done { ResolutionStatus::Executed } else { ResolutionStatus::InProgress })
                .bind(done.then_some(executed_at))
                .bind(created_at)
                .execute(&mut *tx)
                .await?;
                resolutions.push((res_id, *executor_id, done));
            }
            for (res_id, executor_id, _) in &resolutions {
                log(&mut tx, doc_id, ev("resolution_created", manager, created_at, None, None, None,
                    Some(json!({"resolution_id": res_id.to_string(), "executor_id": executor_id.to_string()})))).await?;
            }
            log(&mut tx, doc_id, ev("status_changed", manager, created_at, Some(S::UnderReview), Some(S::InExecution),
                                    Some("Создана резолюция"), None)).await?;
            for (res_id, executor_id, done) in &resolutions {
                if *done {
                    log(&mut tx, doc_id, ev("resolution_executed", *executor_id, executed_at, None, None,
                        Some("Исполнено, отчет подготовлен"), Some(json!({"resolution_ids": [res_id.to_string()]})))).await?;
                }
            }
            if rank >= 3 {
                log(&mut tx, doc_id, ev("status_changed", assigned[0].0, executed_at, Some(S::InExecution),
                                        Some(S::Executed), Some("Поручение исполнено"), None)).await?;
            }
            if rank >= 4 {
                log(&mut tx, doc_id, ev("status_changed", clerk, executed_at + Duration::days(1), Some(S::Executed),
                                        Some(S::RemovedFromControl), Some("Снято с контроля"), None)).await?;
            }
            if let Some(archived) = archived_at.filter(|_| rank >= 5) {
                log(&mut tx, doc_id, ev("status_changed", clerk, archived, Some(S::RemovedFromControl),
                                        Some(S::Archived), Some("Передано в архив"), None)).await?;
            }
        }
    }
    tx.commit().await?;
    println!(
        "Готово: {} пользователей, {} корреспондентов, {} документов.",
        users.len(),
        correspondents.len(),
        total
    );
    Ok(())
}
