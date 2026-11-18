# Информационная система учета входящей корреспонденции

REST API для централизованного электронного учета входящих документов организации: регистрация с автоматическим номером, поиск и фильтрация, резолюции руководителя, назначение исполнителей, контроль сроков, вложения в файловом хранилище SeaweedFS, журнал действий, архив, PDF-отчеты и CSV-выгрузка журнала регистрации. Права доступа проверяются на backend. Backend написан на **Rust**. Веб-интерфейс (HTML + Vanilla JS, без сборки) раздается тем же сервером по адресу **`/ui`** и работает только через REST API.

## Стек

| Уровень | Технологии |
|---|---|
| Backend | Rust (edition 2024), axum 0.8, tokio, sqlx 0.8 (PostgreSQL, встроенные миграции), serde |
| Веб-интерфейс | HTML, CSS, Vanilla JS (ES-модули), Fetch API; без npm, сборщиков и фреймворков; раздается через `tower-http::ServeDir` |
| Данные | PostgreSQL 18; файлы — **SeaweedFS** (S3 API) через `aws-sdk-s3` |
| Безопасность | JWT (`jsonwebtoken`), argon2id (`argon2`), RBAC на экстракторах axum |
| Отчеты | `genpdf` (PDF, шрифт DejaVu Sans с кириллицей), CSV (UTF-8 BOM, `;`) |
| Документация API | OpenAPI 3 (`utoipa`), Swagger UI на `/docs` |
| Инфраструктура | Docker (многоэтапная сборка), Docker Compose, volumes, healthcheck, `restart: unless-stopped` |
| Качество | `cargo test` (unit, HTTP, интеграционные с PostgreSQL и SeaweedFS), `cargo clippy` |

Документация: [архитектура](docs/architecture.md) · [варианты использования](docs/use-case.md) · [состояния](docs/state-diagram.md) · [схема данных](docs/data-schema.md) · [тест-кейсы](docs/test-cases.md) · [допущения](docs/assumptions.md).

## Требования к окружению

- Docker Engine 24+ и плагин Docker Compose v2 (`docker compose version`);
- на Windows — Docker Desktop с включенной виртуализацией (WSL 2 / «Платформа виртуальной машины»);
- свободные порты 8000 (API), а также 5432, 8333, 8888 на `127.0.0.1`;
- 2 ГБ RAM, 3 ГБ диска (сборка Rust-образа).

**AlmaLinux 10:**

```bash
sudo dnf config-manager --add-repo https://download.docker.com/linux/rhel/docker-ce.repo
sudo dnf install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
sudo systemctl enable --now docker          # Docker и контейнеры поднимаются после перезагрузки
sudo usermod -aG docker "$USER"             # затем перелогиниться
sudo firewall-cmd --permanent --add-port=8000/tcp && sudo firewall-cmd --reload
```

SELinux можно оставить в режиме enforcing: bind-mount каталога `backups` помечен `:z`.

## Запуск через Docker

```bash
cp .env.example .env
# обязательно замените пароли, ключи хранилища и JWT_SECRET, например:
openssl rand -base64 48

docker compose up -d --build
docker compose ps                          # backend, postgres, seaweedfs — healthy
curl http://localhost:8000/health          # {"status":"ok","database":"ok","storage":"ok"}
```

Первая сборка образа backend компилирует зависимости Rust и занимает несколько минут; последующие сборки используют кэш слоя зависимостей.

- Веб-интерфейс: **http://localhost:8000/ui** (адрес `http://localhost:8000/` перенаправляет туда же). Вход — учетными записями из seed (см. ниже).
- Swagger UI: **http://localhost:8000/docs**, схема OpenAPI — `/openapi.json`. Вход: `POST /api/v1/auth/login` → скопировать `access_token` → кнопка **Authorize**.
- Health-check: **http://localhost:8000/health** (также `/api/v1/health`).
- Веб-интерфейс filer SeaweedFS (просмотр файлов хранилища): http://127.0.0.1:8888.

Данные хранятся в именованных томах `postgres_data` и `seaweedfs_data` и переживают `docker compose restart`, `docker compose down` (без `-v`) и перезагрузку хоста. Backend корректно завершает работу по SIGTERM (`docker compose stop`): новые соединения не принимаются, текущие запросы дорабатывают.

## Файловое хранилище SeaweedFS

Вложения хранятся в [SeaweedFS](https://github.com/seaweedfs/seaweedfs) — распределенном хранилище объектов (Apache 2.0). Контейнер `seaweedfs` запускает в одном процессе master, volume-сервер, filer и S3-шлюз (`weed server -s3`). Backend обращается к нему по S3 API (порт 8333) с ключами `S3_ACCESS_KEY` / `S3_SECRET_KEY`: при старте контейнера они записываются в конфигурацию identities SeaweedFS. Бакет `S3_BUCKET` backend создает сам при запуске.

Ключ объекта генерирует сервер: `documents/<id документа>/<uuid>.<ext>`; исходное имя файла хранится только в БД. Хранилище наружу не публикуется (порты привязаны к `127.0.0.1`), файлы отдаются только через `GET /api/v1/files/{id}/download` после проверки прав.

## Миграции

Миграции (`migrations/*.sql`) встраиваются в бинарный файл и применяются автоматически при каждом старте backend. Ручной запуск:

```bash
docker compose exec backend correspondence migrate
```

Миграция идемпотентна: база, созданная прежней Python-версией (Alembic), подхватывается без пересоздания.

## Seed-данные

```bash
docker compose exec backend correspondence seed
```

Seed создает 3 подразделения, 5 должностей, 8 типов документов, 10 корреспондентов, 5 пользователей и 20 документов во всех статусах: с резолюциями, историей, просроченными и близкими к сроку. Повторный запуск ничего не дублирует. Все персональные данные вымышлены.

### Тестовые учетные записи

| Логин | Пароль | Роль |
|---|---|---|
| `admin` | `admin123` | Администратор |
| `clerk` | `clerk123` | Делопроизводитель |
| `manager` | `manager123` | Руководитель |
| `executor1` | `executor123` | Исполнитель |
| `executor2` | `executor123` | Исполнитель |

## Веб-интерфейс

Открывается по адресу **http://localhost:8000/ui** после `docker compose up -d` и загрузки seed-данных. Отдельный контейнер, Node.js и сборка не нужны: каталог `ui/` копируется в образ backend, и сервер раздает его как статику (`src/api/mod.rs`, `nest_service("/ui", ServeDir::new(...))`). Swagger остается на `/docs`. REST API не изменился, поэтому интерфейс работает с Rust-версией без правок.

```
ui/
├── index.html   каркас: форма входа, шапка, меню, область содержимого, контейнеры уведомлений и диалогов
├── api.js       HTTP-клиент: JWT в заголовке Authorization, разбор ошибок {detail, code, field}, обработка 401, скачивание файлов
├── app.js       маршрутизация по #-адресам, экраны ролей, формы, валидация, уведомления, диалоги подтверждения
└── styles.css   оформление
```

**Авторизация.** Логин и пароль отправляются в `POST /api/v1/auth/login`. В `localStorage` сохраняется только JWT, пароль не сохраняется нигде. Токен подставляется в каждый запрос как `Authorization: Bearer …`. Любой ответ 401 очищает токен и возвращает на форму входа; после повторного входа открывается тот же раздел. Кнопка «Выйти» находится в шапке.

**Разграничение доступа.** Меню и кнопки строятся по роли и списку разрешений из `GET /api/v1/auth/me`. Это только удобство: скрытая кнопка не защищает. Все проверки прав и бизнес-правил выполняет backend, а интерфейс показывает его ответ.

### Экраны по ролям

| Роль | Разделы меню | Действия в интерфейсе |
|---|---|---|
| Делопроизводитель (`clerk`) | Журнал документов, Регистрация документа, Корреспонденты, Отчёты и выгрузка | поиск, фильтры (статус, тип, корреспондент, даты, контроль срока), сортировка, пагинация; регистрация с отдельной кнопкой «Прикрепить скан»; редактирование реквизитов; загрузка и скачивание файлов; смена статуса: зарегистрирован → на рассмотрении, исполнен → снят с контроля, снят с контроля → в архиве (с подтверждением); корреспонденты: список, поиск, создание, редактирование; три PDF-отчета; CSV-выгрузка журнала |
| Руководитель (`manager`) | Журнал документов, Отчёты | поиск и фильтры (в том числе по исполнителю), карточка, «Добавить резолюцию» (исполнитель из `GET /users/executors`), «Изменить резолюцию», просмотр резолюций, PDF-отчеты |
| Исполнитель (`executor`) | Мои поручения | только документы, где он назначен исполнителем; карточка с резолюцией; загрузка отчетных материалов; «Отметить исполнение»; история |
| Администратор (`admin`) | Документы, Пользователи, Справочники | пользователи: список, фильтры, создание, редактирование, деактивация с подтверждением; справочники: подразделения, должности, типы документов; карточки только для просмотра, открытие по регистрационному номеру, аннулирование и «Удалить карточку» с подтверждением |

Карточка документа содержит вкладки «Основные сведения», «Вложения» (кнопка «Скачать» у каждого файла), «Резолюции», «История», «Управление статусом». В журнале просроченные документы выделены красным, документы с близким сроком — желтым. Признак берется из поля `deadline_state` ответа API и дублируется текстом в колонке «Срок» («просрочен на 3 дня», «срок близок: осталось 2 дня»). Статусы показываются текстом и цветом.

Некоторые справочники роли не выдаются API, поэтому соответствующих фильтров у нее нет. Руководителю недоступен список корреспондентов: он ищет корреспондента через строку поиска, которая охватывает и наименование корреспондента. Делопроизводителю недоступен список исполнителей. У исполнителя и администратора поиска по журналу нет; администратор открывает карточку по точному регистрационному номеру.

### Ошибки и проверки

| Ответ API | Что показывает интерфейс |
|---|---|
| 401 | очистка токена, форма входа с сообщением о завершении сессии |
| 403 | уведомление «Нет доступа»; для деактивированной учетной записи — выход и сообщение на форме входа |
| 404 | уведомление «Не найдено» |
| 409 | уведомление с текстом `detail`; если в ответе есть `field`, поле подсвечивается |
| 413 | уведомление «Файл слишком большой» |
| 422 | подсветка поля из `field` и текст `detail` рядом с ним |

Перед отправкой формы интерфейс проверяет обязательные поля, формат email, ИНН (10 или 12 цифр), `page_count ≥ 1`, срок исполнения не раньше даты регистрации, тип файла (PDF, JPG, PNG, DOCX) и размер до 20 МБ. Эти проверки только ускоряют обратную связь, окончательное решение принимает сервер. Каждая загрузка данных показывает индикатор, пустой результат — «Нет данных» с подсказкой, ошибка загрузки — кнопку «Повторить».

Интерфейс рассчитан на современные браузеры с поддержкой ES-модулей (Chrome, Edge, Firefox, Safari последних версий) и ширину окна от 1280 пикселей; на более узких экранах меню переносится наверх.

## Матрица ролей и разрешений

Единственный источник истины — `src/security.rs` (`role_permissions`).

| Разрешение | Делопроизводитель | Руководитель | Исполнитель | Администратор |
|---|:-:|:-:|:-:|:-:|
| Авторизация | ✅ | ✅ | ✅ | ✅ |
| Пользователи: регистрация, просмотр, редактирование, деактивация | | | | ✅ |
| Справочники: подразделения, должности, типы документов | | | | ✅ |
| Корреспонденты: создание, редактирование, просмотр | ✅ | | | |
| Регистрация карточки | ✅ | | | |
| Редактирование карточки (до архива) | ✅ | | | |
| Просмотр карточки, истории, файлов | ✅ | ✅ | ✅ свои¹ | ✅ чтение² |
| Поиск и фильтрация | ✅ | ✅ | | |
| Удаление карточки | | | | ✅ |
| Создание и редактирование резолюции | | ✅ | | |
| Просмотр резолюций | ✅ | ✅ | ✅ свои¹ | |
| Прикрепление файла | ✅ скан | | ✅ отчет¹ | |
| Смена статуса | ✅ | авто³ | ✅ исполнение¹ | ✅ аннулирование |
| Формирование отчета (PDF) | ✅ | ✅ | | |
| Выгрузка журнала (CSV) | ✅ | | | |

¹ Только документы, по которым пользователю назначена резолюция (контекстная проверка в сервисе).
² Администратор видит карточки только для того, чтобы проверить документ перед удалением или аннулированием; поиск ему недоступен ([допущение A-2](docs/assumptions.md)).
³ «На рассмотрении → на исполнении» выполняется автоматически при создании резолюции.

Коды ответов: 401 — нет или недействительный токен; 403 — роль или контекст не позволяет действие (или пользователь деактивирован); 404 — объект не найден; 405 — метод не поддерживается (например, изменение истории); 409 — недопустимый переход статуса, дубликат, архив; 413 — файл больше лимита; 422 — ошибка валидации. Права проверяются до разбора тела запроса, поэтому 401/403 имеют приоритет над 422. Формат ошибки единый:

```json
{"detail": "Переход «зарегистрирован» → «исполнен» недопустим", "code": "INVALID_STATUS_TRANSITION", "field": "status"}
```

## API (префикс `/api/v1`)

| Метод и путь | Назначение | Роли |
|---|---|---|
| `POST /auth/login` | Вход, JWT, данные пользователя и роль | все |
| `GET /auth/me` | Текущий пользователь и его разрешения | все |
| `POST /users` · `GET /users` · `GET /users/{id}` · `PATCH /users/{id}` · `POST /users/{id}/deactivate` | Управление пользователями | admin |
| `GET /users/executors` | Активные исполнители для назначения резолюций | manager, admin |
| `GET/POST /departments`, `PATCH /departments/{id}` | Подразделения | admin |
| `GET/POST /positions`, `PATCH /positions/{id}` | Должности | admin |
| `GET /document-types` · `POST` · `PATCH /{id}` | Типы документов (чтение — все) | admin |
| `POST /correspondents` · `GET /correspondents` · `GET /{id}` · `PATCH /{id}` | Корреспонденты | clerk |
| `POST /incoming` | Регистрация: номер и дата присваиваются сервером | clerk |
| `GET /incoming` | Список, поиск, фильтры, пагинация, сортировка | все (с учетом прав) |
| `GET /incoming/{id}` · `GET /incoming/by-registration-number/{номер}` | Карточка: реквизиты, резолюции, вложения, история, сроки | все (с учетом прав) |
| `PATCH /incoming/{id}` | Редактирование (кроме архива) | clerk |
| `DELETE /incoming/{id}` | Удаление (ошибка, дубликат) | admin |
| `POST /incoming/{id}/status` | Смена статуса по матрице переходов | clerk, executor, admin |
| `GET /incoming/{id}/history` | История действий | все (с учетом прав) |
| `POST /incoming/{id}/resolutions` · `GET /incoming/{id}/resolutions` | Резолюции | manager / просмотр |
| `PATCH /resolutions/{id}` | Редактирование резолюции | manager |
| `POST /incoming/{id}/files` | Файл PDF/JPG/PNG/DOCX, multipart (`file`, `attachment_type`) | clerk, executor |
| `GET /files/{id}/download` | Скачивание (потоком из SeaweedFS, после RBAC) | все (с учетом прав) |
| `POST /reports/generate` | PDF: `documents`, `executors`, `deadlines` | clerk, manager |
| `GET /exports/registration-log` | CSV журнала регистрации | clerk |
| `GET /health` | БД, файловое хранилище, сервис | без авторизации |

Параметры `GET /incoming`: `page`, `page_size` (≤ 100), `sort` (например, `-registration_date,registration_number`), `registration_number`, `date_from`, `date_to` (по дате регистрации), `correspondent_id`, `executor_id`, `status`, `document_type_id`, `search` (ключевые слова), `deadline_state` (`overdue` | `warning`).

## Пример сценария через curl

```bash
API=http://localhost:8000/api/v1
tok() { curl -s -X POST $API/auth/login -H 'Content-Type: application/json' \
          -d "{\"login\":\"$1\",\"password\":\"$2\"}" | jq -r .access_token; }
CLERK=$(tok clerk clerk123); MANAGER=$(tok manager manager123)

CORR=$(curl -s $API/correspondents -H "Authorization: Bearer $CLERK" | jq -r '.items[0].id')
TYPE=$(curl -s $API/document-types -H "Authorization: Bearer $CLERK" | jq -r '.[0].id')

DOC=$(curl -s -X POST $API/incoming -H "Authorization: Bearer $CLERK" -H 'Content-Type: application/json' -d "{
  \"received_date\":\"$(date +%F)\", \"correspondent_id\":\"$CORR\", \"summary\":\"Запрос сведений\",
  \"document_type_id\":\"$TYPE\", \"page_count\":2, \"execution_deadline\":\"$(date -d '+10 day' +%F)\"}" | jq -r .id)

curl -s -X POST $API/incoming/$DOC/files -H "Authorization: Bearer $CLERK" -F 'file=@scan.pdf;type=application/pdf'
curl -s "$API/exports/registration-log" -H "Authorization: Bearer $CLERK" -o journal.csv
curl -s -X POST $API/reports/generate -H "Authorization: Bearer $MANAGER" -H 'Content-Type: application/json' \
     -d "{\"report_type\":\"executors\",\"period_from\":\"$(date -d '-60 day' +%F)\",\"period_to\":\"$(date +%F)\"}" -o report.pdf
```

## Backup и restore

```bash
scripts/backup_db.sh                                     # → backups/db/db_YYYYMMDD_HHMMSS.dump (pg_dump -Fc)
scripts/backup_storage.sh                                # → backups/storage/storage_YYYYMMDD_HHMMSS/ (rclone copy)

scripts/restore_db.sh backups/db/db_20260930_120000.dump            # пересоздает БД, backend на время останавливается
scripts/restore_storage.sh backups/storage/storage_20260930_120000
```

Копирование файлов выполняет контейнер `rclone` (профиль `tools`) по S3 API SeaweedFS; ключи доступа передаются ему через переменные окружения, а не аргументы командной строки. Копия — обычный каталог с файлами по ключам `documents/<id>/…`, поэтому `restore_storage.sh` подходит и для переноса вложений из копии MinIO, сделанной прежней версией системы.

**Проверка восстановления на чистой среде:**

```bash
scripts/backup_db.sh && scripts/backup_storage.sh
docker compose down -v                                  # удалить контейнеры и тома (данные исчезнут)
docker compose up -d --wait                             # чистая система, миграции применены
scripts/restore_db.sh backups/db/<файл>.dump
scripts/restore_storage.sh backups/storage/<каталог>
# проверить: вход clerk/clerk123, GET /api/v1/incoming — прежние документы;
# GET /api/v1/files/{id}/download — прежние файлы скачиваются
```

Для регулярного копирования добавьте оба скрипта в cron хоста, например `0 2 * * * cd /opt/correspondence && scripts/backup_db.sh && scripts/backup_storage.sh`.

## Тесты

```bash
docker compose up -d --wait postgres seaweedfs
docker compose --profile test run --rm tests                                   # все тесты, включая интеграционные
docker compose --profile test run --rm tests cargo test --release --lib        # только unit
docker compose --profile test run --rm tests cargo clippy --release --all-targets   # линтер
```

| Набор | Файлы | Инфраструктура |
|---|---|---|
| Unit (33) | модули `src/**` (`#[cfg(test)]`): машина состояний, сроки, номер, статистика отчетов, права, JWT, argon2, проверка файлов, CSV, PDF | не нужна |
| HTTP (6) | `tests/http.rs`: раздача `/ui`, Swagger и OpenAPI, 401/405/404/422 в едином формате, `/health` → 503 без зависимостей | не нужна |
| Документация (2) | `tests/docs.rs`: диаграмма состояний = машина состояний, ER-диаграмма = миграции | не нужна |
| Интеграционные (47) | `tests/api.rs`: REST API с настоящими PostgreSQL и SeaweedFS | `#[ignore]` по умолчанию, запускаются в контейнере `tests` |

Интеграционные тесты сами создают отдельную БД `<POSTGRES_DB>_test` и бакет `correspondence-test` (переменная `TEST_S3_BUCKET`), применяют миграции и очищают данные перед каждым тестом; выполняются последовательно. Рабочие данные не затрагиваются. Описание кейсов — в [docs/test-cases.md](docs/test-cases.md).

**Локальная разработка без контейнера backend** (Rust 1.88+):

```bash
docker compose up -d postgres seaweedfs
# в .env заменить хосты postgres/seaweedfs на localhost
cargo run                                   # миграции + сервер на http://localhost:8000
cargo run -- seed                           # демонстрационные данные
cargo test                                  # unit, HTTP и проверки документации
cargo test -- --include-ignored --test-threads=1   # плюс интеграционные
```

## Переменные окружения

| Переменная | Назначение | По умолчанию |
|---|---|---|
| `DATABASE_URL` | `postgres://…` (формат `postgresql+asyncpg://…` прежней версии тоже принимается) | — (обязательна) |
| `JWT_SECRET` | секрет подписи JWT (≥ 16 символов, рекомендуется ≥ 32) | — (обязательна) |
| `ACCESS_TOKEN_EXPIRE_MINUTES` | срок жизни токена | 60 |
| `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_BUCKET`, `S3_REGION` | SeaweedFS (S3 API) | — / `us-east-1` |
| `MAX_UPLOAD_SIZE_MB` | лимит размера файла | 20 |
| `REG_NUMBER_TEMPLATE` | шаблон номера | `ВХ-{YYYY}-{NNNNNN}` |
| `DEADLINE_WARNING_DAYS` | за сколько дней предупреждать о сроке | 3 |
| `APP_TIMEZONE` | часовой пояс организации | `Europe/Moscow` |
| `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` | инициализация контейнера PostgreSQL | — |
| `LOG_LEVEL`, `PDF_FONT_PATH` | уровень логов, путь к TTF для PDF | `INFO`, автопоиск DejaVu |
| `BIND_ADDR`, `UI_DIR` | адрес HTTP-сервера, каталог веб-интерфейса | `0.0.0.0:8000`, `ui` |
| `SEAWEEDFS_VERSION`, `RCLONE_VERSION` | теги образов в docker-compose.yml | `latest` |

Секреты хранятся только в `.env`: файл исключен из git и из Docker-образа.
