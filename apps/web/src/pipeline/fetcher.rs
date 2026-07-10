use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("url `{0}` could not be parsed")]
    InvalidUrl(String),
    #[error("domain `{0}` is not allowlisted for this municipality")]
    NotAllowlisted(String),
    #[error("municipality {0} not found")]
    MunicipalityNotFound(Uuid),
    #[error("http error after retries: {0}")]
    Http(#[from] reqwest::Error),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum FetchOutcome {
    Fetched { document_id: Uuid },
    Duplicate { document_id: Uuid },
}

const MAX_ATTEMPTS: u32 = 5;

pub struct Fetcher {
    client: reqwest::Client,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Fetch `url` for `municipality_id`, enforcing the domain allowlist and
    /// deduping by checksum against previously stored `source_documents`.
    pub async fn fetch(
        &self,
        pool: &PgPool,
        municipality_id: Uuid,
        url: &str,
    ) -> Result<FetchOutcome, FetchError> {
        let parsed = reqwest::Url::parse(url).map_err(|_| FetchError::InvalidUrl(url.to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| FetchError::InvalidUrl(url.to_string()))?
            .to_string();

        let allowlist: Vec<String> = sqlx::query_scalar!(
            "SELECT domain_allowlist FROM municipalities WHERE id = $1",
            municipality_id
        )
        .fetch_optional(pool)
        .await?
        .ok_or(FetchError::MunicipalityNotFound(municipality_id))?;

        let allowed = allowlist
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
        if !allowed {
            return Err(FetchError::NotAllowlisted(host));
        }

        let body = self.fetch_with_retry(url).await?;
        let checksum = format!("{:x}", Sha256::digest(body.as_bytes()));

        if let Some(existing_id) = sqlx::query_scalar!(
            "SELECT id FROM source_documents WHERE municipality_id = $1 AND checksum = $2",
            municipality_id,
            checksum
        )
        .fetch_optional(pool)
        .await?
        {
            return Ok(FetchOutcome::Duplicate {
                document_id: existing_id,
            });
        }

        let document_id = sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum) \
             VALUES ($1, $2, $3) RETURNING id",
            municipality_id,
            url,
            checksum
        )
        .fetch_one(pool)
        .await?;

        Ok(FetchOutcome::Fetched { document_id })
    }

    /// GET `url` with exponential backoff on transient (5xx / network) failures.
    /// Returns the response body once a non-5xx response is received, or the
    /// last error after `MAX_ATTEMPTS` attempts.
    async fn fetch_with_retry(&self, url: &str) -> Result<String, reqwest::Error> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.client.get(url).send().await {
                Ok(resp) if resp.status().is_server_error() && attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Ok(resp) => return resp.error_for_status()?.text().await,
                Err(_err) if attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(50u64 * 2u64.pow(attempt))
}
