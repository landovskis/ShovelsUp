use std::time::Duration;

use serde::Deserialize;

use super::schema::extraction_json_schema;
use super::prompts::en::PROMPT_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("request failed after retries: {0}")]
    RequestFailed(String),
    #[error("model refused the request")]
    Refused,
    #[error("response had no text content")]
    EmptyResponse,
}

/// LLM provider is not specified by the PRD (Autonomous Execution Notes) —
/// `extract_entities` depends on this trait, not a concrete provider, so the
/// model can be swapped without touching call sites.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Sends `system` + `user_content` and returns the raw JSON text of the
    /// model's structured-output response.
    async fn complete(&self, system: &str, user_content: &str) -> Result<String, LlmError>;
}

const MAX_ATTEMPTS: u32 = 5;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-opus-4-8";

const ANTHROPIC_API_BASE_URL: &str = "https://api.anthropic.com";

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: ANTHROPIC_API_BASE_URL.to_string(),
        }
    }

    pub fn from_env() -> Self {
        Self::new(std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set"))
    }

    /// Overrides the API base URL — used in tests to point at a wiremock
    /// server so retry/backoff (TC-REQ-003-5) is exercised deterministically
    /// against the real request/retry code path, not a reimplementation of it.
    #[cfg(test)]
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
        }
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, system: &str, user_content: &str) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": MODEL,
            "max_tokens": 1024,
            "system": system,
            "output_config": {
                "effort": "high",
                "format": {
                    "type": "json_schema",
                    "schema": extraction_json_schema(),
                }
            },
            "messages": [
                { "role": "user", "content": user_content }
            ]
        });

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let response = self
                .client
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_server_error() && attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Ok(resp) if resp.status().is_success() => {
                    let parsed: MessagesResponse = resp
                        .json()
                        .await
                        .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
                    return extract_text(parsed);
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    return Err(LlmError::RequestFailed(format!(
                        "http {status}: {body_text}"
                    )));
                }
                Err(_err) if attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
                Err(err) => return Err(LlmError::RequestFailed(err.to_string())),
            }
        }
    }
}

fn extract_text(response: MessagesResponse) -> Result<String, LlmError> {
    if response.stop_reason.as_deref() == Some("refusal") {
        return Err(LlmError::Refused);
    }
    response
        .content
        .into_iter()
        .find(|block| block.block_type == "text")
        .and_then(|block| block.text)
        .ok_or(LlmError::EmptyResponse)
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(200u64 * 2u64.pow(attempt))
}

/// Marker to keep `PROMPT_VERSION` linked to the request path for future
/// logging/observability (e.g. attaching it to stored extractions) without
/// an unused-import warning today.
pub const _ACTIVE_PROMPT_VERSION: &str = PROMPT_VERSION;

#[cfg(test)]
pub mod test_support {
    use super::{LlmError, LlmProvider};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Returns a fixed response, recording how many times it was called.
    pub struct FixedResponseProvider {
        pub response: String,
        pub call_count: AtomicU32,
    }

    impl FixedResponseProvider {
        pub fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
                call_count: AtomicU32::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for FixedResponseProvider {
        async fn complete(&self, _system: &str, _user_content: &str) -> Result<String, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    /// Always fails — for exercising extract_and_store's "reprocessing"
    /// classification without depending on AnthropicProvider's real retry
    /// path (that path is tested directly against wiremock in this module's
    /// own `tests` submodule, not via a `LlmProvider` double).
    pub struct AlwaysFailingProvider;

    #[async_trait::async_trait]
    impl LlmProvider for AlwaysFailingProvider {
        async fn complete(&self, _system: &str, _user_content: &str) -> Result<String, LlmError> {
            Err(LlmError::RequestFailed("permanently down".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_success_body() -> serde_json::Value {
        serde_json::json!({
            "content": [{ "type": "text", "text": "{\"has_mention\":false,\"physical_work\":false,\"project_name\":null,\"civic_address\":null,\"project_type\":null,\"scale_units\":null,\"scale_gfa_sqm\":null,\"scale_storeys\":null,\"approval_status_raw\":null}" }],
            "stop_reason": "end_turn"
        })
    }

    /// TC-REQ-003-5: LLM 503 retried, succeeds on 3rd attempt — exercised
    /// against AnthropicProvider's real request/retry code, not a
    /// reimplementation of it.
    #[tokio::test]
    async fn complete_retries_503_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fake_success_body()))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("test-key".to_string(), server.uri());
        let result = provider.complete("system", "user").await;
        assert!(result.is_ok(), "expected success after retries, got {result:?}");
        assert!(result.unwrap().contains("has_mention"));
    }

    #[tokio::test]
    async fn complete_gives_up_after_max_attempts_on_sustained_503() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("test-key".to_string(), server.uri());
        let result = provider.complete("system", "user").await;
        assert!(matches!(result, Err(LlmError::RequestFailed(_))));
    }

    #[tokio::test]
    async fn complete_does_not_retry_4xx_responses() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .expect(1)
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("test-key".to_string(), server.uri());
        let result = provider.complete("system", "user").await;
        assert!(matches!(result, Err(LlmError::RequestFailed(_))));
    }

    #[tokio::test]
    async fn complete_surfaces_refusal_as_refused_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [],
                "stop_reason": "refusal"
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("test-key".to_string(), server.uri());
        let result = provider.complete("system", "user").await;
        assert!(matches!(result, Err(LlmError::Refused)));
    }

    #[tokio::test]
    async fn complete_errors_on_empty_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [],
                "stop_reason": "end_turn"
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("test-key".to_string(), server.uri());
        let result = provider.complete("system", "user").await;
        assert!(matches!(result, Err(LlmError::EmptyResponse)));
    }
}
