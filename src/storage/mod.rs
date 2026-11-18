//! Файловое хранилище вложений: SeaweedFS через S3 API (aws-sdk-s3).
//!
//! SeaweedFS поднимается отдельным контейнером (`weed server -s3`); доступ — по ключам из `.env`.
//! Наружу хранилище не публикуется: файлы отдаются только через backend после проверки прав.

pub mod validation;

use std::time::Duration;

use aws_sdk_s3::Client;
use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;

use crate::config::Settings;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StorageError(pub String);

fn storage_err<E: ProvideErrorMetadata + std::fmt::Debug, R: std::fmt::Debug>(err: SdkError<E, R>) -> StorageError {
    let detail = match &err {
        SdkError::ServiceError(service) => {
            let e = service.err();
            format!("{}: {}", e.code().unwrap_or("ServiceError"), e.message().unwrap_or(""))
        }
        other => format!("{other:?}"),
    };
    StorageError(detail)
}

fn is_not_found<E, R>(err: &SdkError<E, R>) -> bool
where
    E: ProvideErrorMetadata,
    R: std::fmt::Debug,
{
    match err {
        SdkError::ServiceError(service) => {
            matches!(service.err().code(), Some("NotFound" | "NoSuchKey" | "NoSuchBucket" | "404"))
                || format!("{:?}", service.raw()).contains("StatusCode(404)")
        }
        _ => false,
    }
}

#[derive(Clone)]
pub struct Storage {
    client: Client,
    bucket: String,
}

impl Storage {
    pub fn new(settings: &Settings) -> Self {
        let credentials =
            Credentials::new(settings.s3_access_key.clone(), settings.s3_secret_key.clone(), None, None, "env");
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(settings.s3_region.clone()))
            .endpoint_url(settings.s3_endpoint.clone())
            .credentials_provider(credentials)
            .force_path_style(true)
            // контрольные суммы — только когда их требует операция (совместимость с S3-шлюзами не от AWS)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(5))
                    .read_timeout(Duration::from_secs(30))
                    .build(),
            )
            .retry_config(RetryConfig::standard().with_max_attempts(2))
            .build();
        Self { client: Client::from_conf(config), bucket: settings.s3_bucket.clone() }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Создает бакет, если его еще нет.
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        match self.client.head_bucket().bucket(&self.bucket).send().await {
            Ok(_) => Ok(()),
            Err(err) if is_not_found(&err) => {
                self.client.create_bucket().bucket(&self.bucket).send().await.map_err(storage_err)?;
                tracing::info!("Создан бакет {} в файловом хранилище", self.bucket);
                Ok(())
            }
            Err(err) => Err(storage_err(err)),
        }
    }

    pub async fn check(&self) -> Result<(), StorageError> {
        self.client.head_bucket().bucket(&self.bucket).send().await.map_err(storage_err)?;
        Ok(())
    }

    pub async fn upload(&self, key: &str, data: Bytes, content_type: &str) -> Result<(), StorageError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .content_length(data.len() as i64)
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(storage_err)?;
        Ok(())
    }

    pub async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        match self.client.head_object().bucket(&self.bucket).key(key).send().await {
            Ok(_) => Ok(true),
            Err(err) if is_not_found(&err) => Ok(false),
            Err(err) => Err(storage_err(err)),
        }
    }

    /// Потоковое чтение объекта (без загрузки файла целиком в память).
    pub async fn stream(&self, key: &str) -> Result<ByteStream, StorageError> {
        let object = self.client.get_object().bucket(&self.bucket).key(key).send().await.map_err(storage_err)?;
        Ok(object.body)
    }

    pub async fn read_all(&self, key: &str) -> Result<Bytes, StorageError> {
        let body = self.stream(key).await?;
        let data = body.collect().await.map_err(|e| StorageError(e.to_string()))?;
        Ok(data.into_bytes())
    }

    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.client.delete_object().bucket(&self.bucket).key(key).send().await.map_err(storage_err)?;
        Ok(())
    }
}
