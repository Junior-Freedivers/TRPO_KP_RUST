#!/usr/bin/env bash
# Копия файлового хранилища SeaweedFS (S3 API) через rclone. Использование: scripts/backup_storage.sh
source "$(dirname "${BASH_SOURCE[0]}")/_common.sh"

BUCKET="$(env_get S3_BUCKET)"
NAME="storage_$(date +%Y%m%d_%H%M%S)"
mkdir -p "backups/storage/$NAME"

# ключи доступа передаются в контейнер rclone через окружение (см. docker-compose.yml), не через аргументы
docker compose up -d --wait seaweedfs
docker compose --profile tools run --rm rclone copy "src:$BUCKET" "/backups/storage/$NAME"
echo "Копия хранилища создана: backups/storage/$NAME ($(find "backups/storage/$NAME" -type f | wc -l) файлов)"
