#!/usr/bin/env bash
# Восстановление файлов в SeaweedFS. Использование: scripts/restore_storage.sh backups/storage/storage_YYYYMMDD_HHMMSS
# Подходит и для переноса вложений из копии MinIO, сделанной прежней версией (mc mirror): структура ключей та же.
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

DIR="${1:?Укажите каталог копии: scripts/restore_storage.sh backups/storage/<каталог>}"
[ -d "$DIR" ] || { echo "Каталог не найден: $DIR" >&2; exit 1; }
NAME="$(basename "$DIR")"
BUCKET="$(env_get S3_BUCKET)"

docker compose up -d --wait seaweedfs
docker compose --profile tools run --rm rclone mkdir "src:$BUCKET"
docker compose --profile tools run --rm rclone copy "/backups/storage/$NAME" "src:$BUCKET"
echo "Файлы восстановлены из $DIR"
