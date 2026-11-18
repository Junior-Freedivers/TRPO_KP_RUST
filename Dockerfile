# syntax=docker/dockerfile:1

# --- стадия сборки --------------------------------------------------------------------------------
FROM rust:1-slim-trixie AS builder
# cmake/perl — сборка aws-lc-sys (TLS для клиента S3); fonts-dejavu-core — шрифт для PDF в тестах
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake perl pkg-config fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build

# 1) зависимости собираются отдельным слоем: он пересобирается только при изменении Cargo.toml/Cargo.lock
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

# 2) исходный код приложения (миграции встраиваются в бинарный файл на этапе компиляции)
COPY src src
COPY migrations migrations
RUN touch src/main.rs src/lib.rs && cargo build --release --locked


# --- рабочий образ --------------------------------------------------------------------------------
FROM debian:trixie-slim AS runtime

# fonts-dejavu-core — шрифт с кириллицей для PDF-отчетов; ca-certificates — для S3 по HTTPS
RUN apt-get update \
    && apt-get install -y --no-install-recommends fonts-dejavu-core ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /build/target/release/correspondence /usr/local/bin/correspondence
COPY ui ./ui
ENV UI_DIR=/app/ui \
    BIND_ADDR=0.0.0.0:8000

RUN useradd --create-home --uid 10001 appuser
USER appuser

EXPOSE 8000
# при старте: миграции -> подготовка бакета -> HTTP-сервер; SIGTERM — штатное завершение (graceful shutdown)
ENTRYPOINT ["correspondence"]
CMD ["serve"]


# --- образ для тестов: docker compose --profile test run --rm tests -------------------------------
FROM builder AS dev
COPY tests tests
COPY docs docs
COPY ui ui
ENV UI_DIR=/build/ui
# unit-тесты + интеграционные (реальные PostgreSQL и SeaweedFS); интеграционные выполняются последовательно
CMD ["cargo", "test", "--release", "--locked", "--", "--include-ignored", "--test-threads=1"]
