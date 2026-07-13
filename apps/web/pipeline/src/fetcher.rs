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
            // reqwest itself never follows redirects: a redirect target must
            // be re-checked against the domain allowlist before it's fetched
            // (see fetch_with_retry), so following one at the HTTP-client
            // level would let an allowlisted host redirect the fetcher to an
            // arbitrary (including internal/private) address — an SSRF
            // vector. `fetch_with_retry` follows at most one redirect hop
            // itself, only after re-validating the target host, discovered
            // when Montreal's real document-listing links turned out to be
            // permalink-style redirectors (typeDoc=pv&doc=N -> the real,
            // stable document URL on the same host) rather than direct URLs.
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
        let allowlist = self.check_allowlist(pool, municipality_id, url).await?;

        let (body, content_type) = self.fetch_with_retry(url, &allowlist).await?;
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

    /// Fetches `url`'s raw bytes for `municipality_id`, enforcing the same
    /// domain allowlist as `fetch`, but without persisting a
    /// `source_documents` row. For index/listing pages consumed only to
    /// discover further document links (`worker::core::extract_pv_document_links`),
    /// not decision-bearing documents in their own right.
    pub async fn fetch_bytes(
        &self,
        pool: &PgPool,
        municipality_id: Uuid,
        url: &str,
    ) -> Result<Vec<u8>, FetchError> {
        let allowlist = self.check_allowlist(pool, municipality_id, url).await?;
        let (body, _content_type) = self.fetch_with_retry(url, &allowlist).await?;
        Ok(body)
    }

    /// Returns the municipality's domain allowlist after confirming `url`'s
    /// host is on it. Callers pass the returned allowlist to
    /// `fetch_with_retry` so a same-domain redirect target can be
    /// re-validated without a second database round trip.
    async fn check_allowlist(
        &self,
        pool: &PgPool,
        municipality_id: Uuid,
        url: &str,
    ) -> Result<Vec<String>, FetchError> {
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

        if !is_allowlisted(&host, &allowlist) {
            return Err(FetchError::NotAllowlisted(host));
        }
        Ok(allowlist)
    }

    /// GET `url` with exponential backoff on transient (5xx / network) failures.
    /// Returns the raw response body (never decoded as text — REQ-002 parses
    /// PDFs, which are binary) plus its declared `Content-Type`, once a
    /// non-5xx response is received, or the last error after `MAX_ATTEMPTS`
    /// attempts.
    ///
    /// Follows at most `MAX_REDIRECTS` redirect hops: some municipal sites
    /// resolve a document through a short chain rather than serving it
    /// directly — Montreal's document permalinks resolve via a 302 to a
    /// canonical URL, which itself 302s from `http://` to `https://` before
    /// serving the real content (2 hops, confirmed directly against the
    /// live site). The redirect target's host is re-validated against
    /// `allowlist` at every hop before being followed — an unvalidated
    /// follow would let an allowlisted host redirect the fetcher to an
    /// arbitrary address (SSRF). Exceeding `MAX_REDIRECTS` is rejected
    /// outright, as is any redirect whose target host isn't allowlisted.
    async fn fetch_with_retry(
        &self,
        url: &str,
        allowlist: &[String],
    ) -> Result<(Vec<u8>, Option<String>), FetchError> {
        const MAX_REDIRECTS: u32 = 2;

        let mut current_url = url.to_string();
        let mut redirects = 0u32;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.client.get(&current_url).send().await {
                Ok(resp) if resp.status().is_server_error() && attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Ok(resp) if resp.status().is_redirection() => {
                    if redirects >= MAX_REDIRECTS {
                        return Err(FetchError::UnexpectedRedirect {
                            url: current_url,
                            status: resp.status().as_u16(),
                        });
                    }

                    let status = resp.status().as_u16();
                    let location = resp
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|v| v.to_str().ok())
                        .ok_or(FetchError::UnexpectedRedirect {
                            url: current_url.clone(),
                            status,
                        })?
                        .to_string();

                    let base = reqwest::Url::parse(&current_url)
                        .map_err(|_| FetchError::InvalidUrl(current_url.clone()))?;
                    let target = base
                        .join(&location)
                        .map_err(|_| FetchError::InvalidUrl(location.clone()))?;
                    let target_host = target
                        .host_str()
                        .ok_or_else(|| FetchError::InvalidUrl(target.to_string()))?;

                    if !is_allowlisted(target_host, allowlist) {
                        return Err(FetchError::NotAllowlisted(target_host.to_string()));
                    }

                    current_url = target.to_string();
                    redirects += 1;
                    attempt = 0;
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
