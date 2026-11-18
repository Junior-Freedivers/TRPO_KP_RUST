-- Начальная схема БД (перенесена из Alembic-ревизии 0001 без изменений).
-- Все операторы идемпотентны (IF NOT EXISTS): миграция безопасно применяется и к базе,
-- созданной прежней Python-версией backend (таблица alembic_version при этом просто не используется).

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE IF NOT EXISTS departments (
    id          uuid PRIMARY KEY,
    name        varchar(255) NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_departments_name UNIQUE (name)
);

CREATE TABLE IF NOT EXISTS positions (
    id    uuid PRIMARY KEY,
    name  varchar(255) NOT NULL,
    CONSTRAINT uq_positions_name UNIQUE (name)
);

CREATE TABLE IF NOT EXISTS document_types (
    id         uuid PRIMARY KEY,
    name       varchar(255) NOT NULL,
    is_active  boolean NOT NULL DEFAULT true,
    CONSTRAINT uq_document_types_name UNIQUE (name)
);

CREATE TABLE IF NOT EXISTS users (
    id             uuid PRIMARY KEY,
    full_name      varchar(255) NOT NULL,
    login          varchar(64) NOT NULL,
    email          varchar(255) NOT NULL,
    password_hash  varchar(255) NOT NULL,
    role           varchar(16) NOT NULL,
    department_id  uuid NOT NULL REFERENCES departments (id) ON DELETE RESTRICT,
    position_id    uuid REFERENCES positions (id) ON DELETE RESTRICT,
    is_active      boolean NOT NULL DEFAULT true,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_users_login UNIQUE (login),
    CONSTRAINT uq_users_email UNIQUE (email),
    CONSTRAINT ck_users_role_valid CHECK (role IN ('clerk', 'manager', 'executor', 'admin'))
);
CREATE INDEX IF NOT EXISTS ix_users_role ON users (role);
CREATE INDEX IF NOT EXISTS ix_users_department_id ON users (department_id);

CREATE TABLE IF NOT EXISTS correspondents (
    id                uuid PRIMARY KEY,
    name              varchar(500) NOT NULL,
    inn               varchar(12),
    address           text,
    phone             varchar(64),
    email             varchar(255),
    signer_full_name  varchar(255),
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_correspondents_name ON correspondents (name);
CREATE INDEX IF NOT EXISTS ix_correspondents_inn ON correspondents (inn);

CREATE TABLE IF NOT EXISTS registration_counters (
    year        integer PRIMARY KEY,
    last_value  integer NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS incoming_documents (
    id                  uuid PRIMARY KEY,
    registration_number varchar(64) NOT NULL,
    received_date       date NOT NULL,
    registration_date   date NOT NULL,
    correspondent_id    uuid NOT NULL REFERENCES correspondents (id) ON DELETE RESTRICT,
    addressee           varchar(255),
    summary             text NOT NULL,
    document_type_id    uuid NOT NULL REFERENCES document_types (id) ON DELETE RESTRICT,
    page_count          integer NOT NULL,
    execution_deadline  date NOT NULL,
    status              varchar(32) NOT NULL,
    created_by          uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    archived_at         timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_incoming_documents_registration_number UNIQUE (registration_number),
    CONSTRAINT ck_incoming_documents_page_count_positive CHECK (page_count >= 1),
    CONSTRAINT ck_incoming_documents_deadline_not_before_registration
        CHECK (execution_deadline >= registration_date),
    CONSTRAINT ck_incoming_documents_status_valid CHECK (status IN (
        'зарегистрирован', 'на рассмотрении', 'на исполнении', 'исполнен', 'снят с контроля', 'в архиве')),
    CONSTRAINT ck_incoming_documents_archived_consistent
        CHECK ((status = 'в архиве') = (archived_at IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_received_date ON incoming_documents (received_date);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_registration_date ON incoming_documents (registration_date);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_correspondent_id ON incoming_documents (correspondent_id);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_document_type_id ON incoming_documents (document_type_id);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_execution_deadline ON incoming_documents (execution_deadline);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_status ON incoming_documents (status);
CREATE INDEX IF NOT EXISTS ix_incoming_documents_created_by ON incoming_documents (created_by);
-- поиск по ILIKE '%...%' (расширение pg_trgm)
CREATE INDEX IF NOT EXISTS ix_incoming_documents_summary_trgm
    ON incoming_documents USING gin (summary gin_trgm_ops);

CREATE TABLE IF NOT EXISTS resolutions (
    id                    uuid PRIMARY KEY,
    document_id           uuid NOT NULL REFERENCES incoming_documents (id) ON DELETE CASCADE,
    text                  text NOT NULL,
    author_id             uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    assigned_executor_id  uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    deadline              date NOT NULL,
    status                varchar(32) NOT NULL,
    executed_at           timestamptz,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT ck_resolutions_status_valid CHECK (status IN ('на исполнении', 'исполнена'))
);
CREATE INDEX IF NOT EXISTS ix_resolutions_document_id ON resolutions (document_id);
CREATE INDEX IF NOT EXISTS ix_resolutions_assigned_executor_id ON resolutions (assigned_executor_id);
CREATE INDEX IF NOT EXISTS ix_resolutions_status ON resolutions (status);

CREATE TABLE IF NOT EXISTS attachments (
    id                 uuid PRIMARY KEY,
    document_id        uuid NOT NULL REFERENCES incoming_documents (id) ON DELETE CASCADE,
    original_filename  varchar(255) NOT NULL,
    storage_key        varchar(512) NOT NULL,
    content_type       varchar(128) NOT NULL,
    size_bytes         integer NOT NULL,
    attachment_type    varchar(32) NOT NULL,
    uploaded_by        uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    created_at         timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_attachments_storage_key UNIQUE (storage_key),
    CONSTRAINT ck_attachments_size_positive CHECK (size_bytes > 0),
    CONSTRAINT ck_attachments_type_valid CHECK (attachment_type IN ('source_scan', 'execution_report', 'other'))
);
CREATE INDEX IF NOT EXISTS ix_attachments_document_id ON attachments (document_id);

CREATE TABLE IF NOT EXISTS document_history (
    id           uuid PRIMARY KEY,
    document_id  uuid NOT NULL REFERENCES incoming_documents (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    action       varchar(64) NOT NULL,
    old_status   varchar(32),
    new_status   varchar(32),
    comment      text,
    metadata     jsonb,
    created_at   timestamptz NOT NULL DEFAULT now(),
    -- монотонный порядковый номер: стабильный порядок записей, созданных в одной транзакции
    seq          bigint GENERATED ALWAYS AS IDENTITY,
    CONSTRAINT uq_document_history_seq UNIQUE (seq),
    CONSTRAINT ck_document_history_old_status_valid CHECK (old_status IS NULL OR old_status IN (
        'зарегистрирован', 'на рассмотрении', 'на исполнении', 'исполнен', 'снят с контроля', 'в архиве')),
    CONSTRAINT ck_document_history_new_status_valid CHECK (new_status IS NULL OR new_status IN (
        'зарегистрирован', 'на рассмотрении', 'на исполнении', 'исполнен', 'снят с контроля', 'в архиве'))
);
CREATE INDEX IF NOT EXISTS ix_document_history_document_id ON document_history (document_id);
