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
    #[error("refused to follow redirect (status {status}) from `{url}`")]
    UnexpectedRedirect { url: String, status: u16 },
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
            // Redirects are not followed: a redirect target is not re-checked
            // against the domain allowlist, so following one would let an
            // allowlisted host redirect the fetcher to an arbitrary
            // (including internal/private) address — an SSRF vector. Municipal
            // agenda pages are fetched by direct URL, so this is not expected
            // to affect legitimate fetches; a redirect response instead
            // surfaces as an HTTP error via `error_for_status`.
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client with no-redirect policy must build"),
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
        let parsed =
            reqwest::Url::parse(url).map_err(|_| FetchError::InvalidUrl(url.to_string()))?;
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

        if !is_allowlisted(&host, &allowlist) {
            return Err(FetchError::NotAllowlisted(host));
        }

        let (body, content_type) = self.fetch_with_retry(url).await?;
        let checksum = format!("{:x}", Sha256::digest(&body));

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
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
            municipality_id,
            url,
            checksum,
            body,
            content_type,
        )
        .fetch_one(pool)
        .await?;

        Ok(FetchOutcome::Fetched { document_id })
    }

    /// GET `url` with exponential backoff on transient (5xx / network) failures.
    /// Returns the raw response body (never decoded as text — REQ-002 parses
    /// PDFs, which are binary) plus its declared `Content-Type`, once a
    /// non-5xx response is received, or the last error after `MAX_ATTEMPTS`
    /// attempts. Redirects are never followed (see the SSRF note on
    /// `Fetcher::new`) and are rejected outright rather than treated as a
    /// successful response.
    async fn fetch_with_retry(&self, url: &str) -> Result<(Vec<u8>, Option<String>), FetchError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.client.get(url).send().await {
                Ok(resp) if resp.status().is_server_error() && attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Ok(resp) if resp.status().is_redirection() => {
                    return Err(FetchError::UnexpectedRedirect {
                        url: url.to_string(),
                        status: resp.status().as_u16(),
                    });
                }
                Ok(resp) => {
                    let resp = resp.error_for_status()?;
                    let content_type = resp
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    let bytes = resp.bytes().await?.to_vec();
                    return Ok((bytes, content_type));
                }
                Err(_err) if attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(50u64 * 2u64.pow(attempt))
}

fn is_allowlisted(host: &str, allowlist: &[String]) -> bool {
    allowlist
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delay_grows_exponentially() {
        assert_eq!(backoff_delay(1), Duration::from_millis(100));
        assert_eq!(backoff_delay(2), Duration::from_millis(200));
        assert_eq!(backoff_delay(3), Duration::from_millis(400));
        assert_eq!(backoff_delay(4), Duration::from_millis(800));
    }

    #[test]
    fn is_allowlisted_matches_exact_domain() {
        let allowlist = vec!["toronto.ca".to_string()];
        assert!(is_allowlisted("toronto.ca", &allowlist));
    }

    #[test]
    fn is_allowlisted_matches_subdomain() {
        let allowlist = vec!["toronto.ca".to_string()];
        assert!(is_allowlisted("app.toronto.ca", &allowlist));
    }

    #[test]
    fn is_allowlisted_rejects_similar_but_different_domain() {
        let allowlist = vec!["toronto.ca".to_string()];
        assert!(!is_allowlisted("not-toronto.ca", &allowlist));
        assert!(!is_allowlisted("toronto.ca.evil.example", &allowlist));
    }

    #[test]
    fn is_allowlisted_rejects_unrelated_domain() {
        let allowlist = vec!["toronto.ca".to_string()];
        assert!(!is_allowlisted("vancouver.ca", &allowlist));
    }

    #[test]
    fn is_allowlisted_empty_list_rejects_everything() {
        assert!(!is_allowlisted("toronto.ca", &[]));
    }
}
