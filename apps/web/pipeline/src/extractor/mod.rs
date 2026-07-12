pub mod llm;
pub(crate) mod prompts;
pub mod schema;
pub(crate) mod scale;
pub(crate) mod validator;

use uuid::Uuid;

use crate::{normalizer, redaction};
use llm::LlmProvider;
use schema::{ExtractionResult, RawExtraction};

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("llm request failed after retries: {0}")]
    Llm(String),
    #[error("llm returned malformed or truncated JSON: {0}")]
    MalformedJson(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Extracts a construction-project mention from `chunk_text`, if any.
/// Applies RULE-001 deterministically over the LLM's own physical_work
/// classification (see `validator` — LLM self-classification cannot be
/// trusted; this is REQ-003's central design decision) and accepts the
/// mention if at least one scale indicator is present (see `scale`).
///
/// Returns `Ok(None)` when the LLM found no project mention in the chunk
/// (not an error). Malformed/truncated JSON is a discardable per-chunk
/// failure (TC-REQ-003-4), not a crash.
/// `language` selects the extraction prompt (IMP-REQ-007-03): `"fr"` routes
/// to `prompts::fr`, anything else (including unset/unknown) defaults to
/// `prompts::en` — RULE-001 validation and scale-indicator acceptance below
/// are language-agnostic and shared across both.
pub async fn extract_entities(
    chunk_text: &str,
    language: &str,
    llm: &dyn LlmProvider,
) -> Result<Option<ExtractionResult>, ExtractError> {
    let system_prompt = match language {
        "fr" => prompts::fr::SYSTEM_PROMPT,
        _ => prompts::en::SYSTEM_PROMPT,
    };
    let raw_json = llm
        .complete(system_prompt, chunk_text)
        .await
        .map_err(|e| ExtractError::Llm(e.to_string()))?;

    let raw: RawExtraction = serde_json::from_str(&raw_json)
        .map_err(|e| ExtractError::MalformedJson(format!("{e}: {raw_json}")))?;

    if !raw.has_mention {
        return Ok(None);
    }

    let physical_work = validator::validate_physical_work(chunk_text, language, raw.physical_work);
    if !physical_work {
        return Ok(None);
    }

    if !scale::has_scale_indicator(raw.scale_units, raw.scale_gfa_sqm, raw.scale_storeys) {
        return Ok(None);
    }

    // IMP-REQ-007-04: strip named individuals the LLM may have captured
    // into project_name (e.g. "Demande de Jean Tremblay pour...") before
    // persisting — a project's *name* should never carry a person's name
    // through to a public-facing record.
    let project_name = match language {
        "fr" => raw.project_name.map(|name| redaction::fr::redact(&name)),
        _ => raw.project_name,
    };

    // Status-recovery second pass (TC-REQ-003-1 field-completeness fix):
    // the main call asks for 9 fields at once, and approval_status_raw —
    // almost always a short trailing sentence separate from the rest of
    // the excerpt — is disproportionately likely to be missed under that
    // load even though the same model reliably finds it when it's the
    // *only* thing being asked for. Only fires when the main pass came back
    // null, so it costs nothing on the common case where the field was
    // already found. `temperature` is not a usable lever here — the
    // Anthropic API rejects it outright as deprecated for this model,
    // confirmed directly against the live API rather than assumed.
    let approval_status_raw = match raw.approval_status_raw {
        Some(status) => Some(status),
        None => recover_status(chunk_text, language, llm).await,
    };

    Ok(Some(ExtractionResult {
        physical_work,
        project_name,
        civic_address: raw.civic_address,
        project_type: raw.project_type,
        scale_units: raw.scale_units,
        scale_gfa_sqm: raw.scale_gfa_sqm,
        scale_storeys: raw.scale_storeys,
        approval_status_raw,
        reference_number: raw.reference_number,
    }))
}

/// Focused second-pass call for `approval_status_raw` alone, used only
/// when the main extraction pass returned `null` for it. Failures here
/// (network error, refusal, an unparseable response) are swallowed to
/// `None` rather than propagated — this is a best-effort completeness
/// improvement, not a required step; the mention still persists without a
/// status either way, exactly as it did before this recovery pass existed.
async fn recover_status(chunk_text: &str, language: &str, llm: &dyn LlmProvider) -> Option<String> {
    let system_prompt = match language {
        "fr" => prompts::fr::STATUS_ONLY_SYSTEM_PROMPT,
        _ => prompts::en::STATUS_ONLY_SYSTEM_PROMPT,
    };

    let response = llm.complete_text(system_prompt, chunk_text).await.ok()?;
    let trimmed = response.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Runs extraction for `document_chunk_id`, persists the resulting mention
/// (if any), and records the outcome on `document_chunks.extraction_status`:
///
/// - A qualifying mention was found: `extracted`, row inserted.
/// - The LLM found nothing, or RULE-001/scale rejected it: `no_mention`.
/// - Malformed/truncated JSON: `failed` — zero rows persisted (TC-REQ-003-4).
/// - LLM transient failure (retries exhausted, IMP-REQ-003-06): `reprocessing`.
///
/// Only a database error surfaces as `Err` — every extraction outcome above
/// is handled and recorded, not propagated as an error.
pub async fn extract_and_store(
    pool: &sqlx::PgPool,
    document_chunk_id: Uuid,
    chunk_text: &str,
    llm: &dyn LlmProvider,
) -> Result<Option<Uuid>, sqlx::Error> {
    // IMP-REQ-007-03: language drives both prompt selection here and status
    // normalization below — fetched once up front rather than twice.
    let chunk_language: Option<String> = sqlx::query_scalar!(
        "SELECT language FROM document_chunks WHERE id = $1",
        document_chunk_id
    )
    .fetch_one(pool)
    .await?;
    let language = chunk_language.as_deref().unwrap_or("en");

    let outcome = extract_entities(chunk_text, language, llm).await;

    let status = match &outcome {
        Ok(Some(_)) => "extracted",
        Ok(None) => "no_mention",
        Err(ExtractError::MalformedJson(_)) => "failed",
        Err(ExtractError::Llm(_)) => "reprocessing",
        Err(ExtractError::Db(_)) => unreachable!("extract_entities never returns Db"),
    };

    let mention_id = match outcome {
        Ok(Some(extraction)) => {
            let mention_id = sqlx::query_scalar!(
                "INSERT INTO project_mentions \
                 (document_chunk_id, physical_work, project_name, civic_address, project_type, \
                  scale_units, scale_gfa_sqm, scale_storeys, approval_status_raw, reference_number) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
                document_chunk_id,
                extraction.physical_work,
                extraction.project_name,
                extraction.civic_address,
                extraction.project_type,
                extraction.scale_units,
                extraction.scale_gfa_sqm,
                extraction.scale_storeys,
                extraction.approval_status_raw,
                extraction.reference_number,
            )
            .fetch_one(pool)
            .await?;

            // REQ-004: normalize the raw status and flag any same-document
            // conflict, wired directly into the extraction output path
            // (IMP-REQ-004-06) rather than as a separate later pass.
            if let Some(raw_status) = &extraction.approval_status_raw {
                if let Some(normalized) =
                    normalizer::normalize_status(pool, raw_status, language).await?
                {
                    sqlx::query!(
                        "UPDATE project_mentions SET normalized_status = $1 WHERE id = $2",
                        normalized,
                        mention_id
                    )
                    .execute(pool)
                    .await?;

                    normalizer::detect_and_flag_status_conflict(pool, mention_id).await?;
                }
            }

            // REQ-005: automatically resolve the new mention into a
            // tracked project (IMP-REQ-005-05). A resolver error is
            // reported here but does not unwind extraction — the mention
            // itself was already persisted successfully.
        if let Err(err) = crate::resolver::resolve_mention(pool, mention_id).await {
                tracing::warn!(
                    mention_id = %mention_id,
                    error = %err,
                    "resolve_mention failed after extraction"
                );
            }

            Some(mention_id)
        }
        _ => None,
    };

    sqlx::query!(
        "UPDATE document_chunks SET extraction_status = $1 WHERE id = $2",
        status,
        document_chunk_id
    )
    .execute(pool)
    .await?;

    Ok(mention_id)
}

#[cfg(test)]
mod tests {
    use super::llm::test_support::{AlwaysFailingProvider, FixedResponseProvider};
    use super::*;
    use sqlx::PgPool;

    const QUALIFYING_JSON: &str = r#"{"has_mention":true,"physical_work":true,"project_name":"Riverside Commons","civic_address":"123 Main St","project_type":"residential","scale_units":48,"scale_gfa_sqm":null,"scale_storeys":6,"approval_status_raw":"Approved"}"#;
    const NO_MENTION_JSON: &str = r#"{"has_mention":false,"physical_work":false,"project_name":null,"civic_address":null,"project_type":null,"scale_units":null,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":null}"#;
    const MALFORMED_JSON: &str = r#"{"has_mention": true, "physical_work": tru"#; // truncated

    #[tokio::test]
    async fn extract_entities_returns_result_for_qualifying_mention() {
        let llm = FixedResponseProvider::new(QUALIFYING_JSON);
        let result = extract_entities(
            "Council approved 48-unit, 6-storey building at 123 Main St.",
            "en",
            &llm,
        )
        .await
        .unwrap();
        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(extraction.project_name.as_deref(), Some("Riverside Commons"));
        assert_eq!(extraction.scale_units, Some(48));
    }

    #[tokio::test]
    async fn extract_entities_returns_none_when_llm_reports_no_mention() {
        let llm = FixedResponseProvider::new(NO_MENTION_JSON);
        let result = extract_entities("The meeting was called to order.", "en", &llm)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    /// TC-REQ-003-4: malformed LLM JSON discarded, not persisted.
    #[tokio::test]
    async fn extract_entities_returns_malformed_json_error() {
        let llm = FixedResponseProvider::new(MALFORMED_JSON);
        let result = extract_entities("Some chunk text.", "en", &llm).await;
        assert!(matches!(result, Err(ExtractError::MalformedJson(_))));
    }

    /// RULE-001 rejects a rezoning-only mention even when the LLM claims
    /// has_mention/physical_work true and supplies a scale indicator.
    #[tokio::test]
    async fn extract_entities_rejects_rezoning_only_despite_llm_claim() {
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":true,"project_name":"X","civic_address":null,"project_type":null,"scale_units":10,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":null}"#,
        );
        let result = extract_entities(
            "Zoning by-law amendment to permit mixed-use designation at 400 King St.",
            "en",
            &llm,
        )
        .await
        .unwrap();
        assert!(result.is_none());
    }

    /// IMP-REQ-007-04: a named individual the LLM captures into
    /// `project_name` is redacted before the `ExtractionResult` is built —
    /// exercised through `extract_entities`, not just the `redaction`
    /// module in isolation, to confirm the wiring actually runs.
    #[tokio::test]
    async fn extract_entities_redacts_named_individual_from_french_project_name() {
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":true,"project_name":"Demande de M. Jean Tremblay","civic_address":"123, rue Principale","project_type":"résidentiel","scale_units":48,"scale_gfa_sqm":null,"scale_storeys":6,"approval_status_raw":"Approuvé"}"#,
        );
        let result = extract_entities(
            "Point 4 : Demande de M. Jean Tremblay pour la construction d'un nouveau bâtiment résidentiel au 123, rue Principale, 48 logements, 6 étages. Approuvé.",
            "fr",
            &llm,
        )
        .await
        .unwrap();
        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(
            extraction.project_name.as_deref(),
            Some("Demande de M. [nom retiré]")
        );
    }

    /// Status-recovery second pass: when the main call returns
    /// `approval_status_raw: null`, a second, status-only call runs and its
    /// result is used instead of leaving the field null.
    #[tokio::test]
    async fn extract_entities_recovers_null_status_via_second_pass() {
        struct SequencedProvider {
            responses: std::sync::Mutex<Vec<&'static str>>,
            prompts_seen: std::sync::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for SequencedProvider {
            async fn complete(
                &self,
                system: &str,
                _user_content: &str,
            ) -> Result<String, llm::LlmError> {
                self.prompts_seen.lock().unwrap().push(system.to_string());
                Ok(self.responses.lock().unwrap().remove(0).to_string())
            }
        }
        let no_status_json = r#"{"has_mention":true,"physical_work":true,"project_name":"Maple Court","civic_address":"123 Main St","project_type":"residential","scale_units":48,"scale_gfa_sqm":null,"scale_storeys":6,"approval_status_raw":null}"#;
        let llm = SequencedProvider {
            responses: std::sync::Mutex::new(vec![no_status_json, "Approved."]),
            prompts_seen: std::sync::Mutex::new(Vec::new()),
        };

        let result = extract_entities(
            "Item 4: construction of a new residential building at 123 Main St, 48 units, 6 storeys. Approved.",
            "en",
            &llm,
        )
        .await
        .unwrap();

        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(extraction.approval_status_raw.as_deref(), Some("Approved."));

        let prompts_seen = llm.prompts_seen.lock().unwrap();
        assert_eq!(prompts_seen.len(), 2, "expected exactly one recovery call, not more");
        assert_eq!(prompts_seen[1], prompts::en::STATUS_ONLY_SYSTEM_PROMPT);
    }

    /// The recovery pass responding "NONE" leaves the field null rather
    /// than persisting the literal word.
    #[tokio::test]
    async fn extract_entities_leaves_status_null_when_recovery_also_finds_none() {
        let no_status_json = r#"{"has_mention":true,"physical_work":true,"project_name":"Maple Court","civic_address":"123 Main St","project_type":"residential","scale_units":48,"scale_gfa_sqm":null,"scale_storeys":6,"approval_status_raw":null}"#;

        struct TwoResponseProvider {
            responses: std::sync::Mutex<Vec<&'static str>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for TwoResponseProvider {
            async fn complete(&self, _system: &str, _user_content: &str) -> Result<String, llm::LlmError> {
                Ok(self.responses.lock().unwrap().remove(0).to_string())
            }
        }
        let llm = TwoResponseProvider {
            responses: std::sync::Mutex::new(vec![no_status_json, "NONE"]),
        };

        let result = extract_entities("Item 4: some project text with no stated decision.", "en", &llm)
            .await
            .unwrap();

        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(extraction.approval_status_raw, None);
    }

    /// TC-REQ-007-1: French proceedings extract all 5 fields at EN parity —
    /// exercises the `"fr"` prompt-routing path end to end (IMP-REQ-007-01/-03).
    #[tokio::test]
    async fn extract_entities_routes_to_french_prompt_and_extracts() {
        struct RecordingProvider {
            response: &'static str,
            system_prompt: std::sync::Mutex<Option<String>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for RecordingProvider {
            async fn complete(
                &self,
                system: &str,
                _user_content: &str,
            ) -> Result<String, llm::LlmError> {
                *self.system_prompt.lock().unwrap() = Some(system.to_string());
                Ok(self.response.to_string())
            }
        }
        let fr_json = r#"{"has_mention":true,"physical_work":true,"project_name":"Les Jardins du Parc","civic_address":"123, rue Principale","project_type":"résidentiel","scale_units":48,"scale_gfa_sqm":null,"scale_storeys":6,"approval_status_raw":"Approuvé"}"#;
        let llm = RecordingProvider {
            response: fr_json,
            system_prompt: std::sync::Mutex::new(None),
        };

        let result = extract_entities(
            "Le conseil a approuvé un bâtiment de 48 logements et 6 étages au 123, rue Principale.",
            "fr",
            &llm,
        )
        .await
        .unwrap();

        assert_eq!(
            result.expect("expected a qualifying extraction").project_name.as_deref(),
            Some("Les Jardins du Parc")
        );
        let used_prompt = llm.system_prompt.lock().unwrap().clone().unwrap();
        assert_eq!(used_prompt, prompts::fr::SYSTEM_PROMPT);
    }

    /// TC-REQ-007-2: a minimal single-word French status phrase round-trips
    /// through extraction into `approval_status_raw` intact — the pipeline
    /// half of this test case. (Whether the live LLM actually *produces* a
    /// single-word status this reliably is verified by the FR field-
    /// completeness gate in tests/pipeline_extraction_fr.rs when
    /// ANTHROPIC_API_KEY is set; this test verifies the code path doesn't
    /// truncate, discard, or otherwise mangle a short value.)
    #[tokio::test]
    async fn extract_entities_preserves_minimal_single_word_french_status() {
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":true,"project_name":null,"civic_address":"200, rue Elm","project_type":"institutionnel","scale_units":null,"scale_gfa_sqm":null,"scale_storeys":2,"approval_status_raw":"Approuvé"}"#,
        );
        let result = extract_entities(
            "Point 9 : Rénovation du centre communautaire au 200, rue Elm, ajout de 2 étages. Approuvé.",
            "fr",
            &llm,
        )
        .await
        .unwrap();
        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(extraction.approval_status_raw.as_deref(), Some("Approuvé"));
    }

    /// TC-REQ-007-3: RULE-001 excludes a French rezoning-only motion even
    /// when the LLM hallucinates physical_work=true.
    #[tokio::test]
    async fn extract_entities_rejects_french_rezoning_only_despite_llm_claim() {
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":true,"project_name":"X","civic_address":null,"project_type":null,"scale_units":10,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":null}"#,
        );
        let result = extract_entities(
            "Modification de zonage pour permettre une désignation à usage mixte au 400, rue King.",
            "fr",
            &llm,
        )
        .await
        .unwrap();
        assert!(result.is_none());
    }

    async fn seed_chunk(pool: &PgPool) -> Uuid {
        let municipality_id = sqlx::query_scalar!(
            "INSERT INTO municipalities (name, slug, domain_allowlist) \
             VALUES ('Test City', 'test-city', ARRAY['test-city.example']) RETURNING id"
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let doc_id = sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, 'https://test-city.example/doc', 'chk', ''::bytea, 'text/html') RETURNING id",
            municipality_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar!(
            "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
             VALUES ($1, 0, 'Council approved 48-unit building at 123 Main St.') RETURNING id",
            doc_id
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

#[sqlx::test(migrations = "../web/migrations")]
    async fn extract_and_store_inserts_mention_and_marks_extracted(pool: PgPool) {
        let chunk_id = seed_chunk(&pool).await;
        let llm = FixedResponseProvider::new(QUALIFYING_JSON);

        let mention_id = extract_and_store(&pool, chunk_id, "chunk text", &llm).await.unwrap();
        assert!(mention_id.is_some());

        let status: String = sqlx::query_scalar!(
            "SELECT extraction_status FROM document_chunks WHERE id = $1",
            chunk_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "extracted");
    }

    /// reference_number (added for REQ-005's cross-reference matcher) must
    /// round-trip through extract_and_store, not just parse.
#[sqlx::test(migrations = "../web/migrations")]
    async fn extract_and_store_persists_reference_number(pool: PgPool) {
        let chunk_id = seed_chunk(&pool).await;
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":true,"project_name":"Riverside Commons","civic_address":"123 Main St","project_type":"residential","scale_units":48,"scale_gfa_sqm":null,"scale_storeys":6,"approval_status_raw":"Approved","reference_number":"Application No. 2026-045"}"#,
        );

        let mention_id = extract_and_store(&pool, chunk_id, "chunk text", &llm)
            .await
            .unwrap()
            .expect("expected a qualifying mention");

        let reference_number: Option<String> = sqlx::query_scalar!(
            "SELECT reference_number FROM project_mentions WHERE id = $1",
            mention_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(reference_number.as_deref(), Some("Application No. 2026-045"));
    }

#[sqlx::test(migrations = "../web/migrations")]
    async fn extract_and_store_marks_no_mention_without_inserting(pool: PgPool) {
        let chunk_id = seed_chunk(&pool).await;
        let llm = FixedResponseProvider::new(NO_MENTION_JSON);

        let mention_id = extract_and_store(&pool, chunk_id, "chunk text", &llm).await.unwrap();
        assert!(mention_id.is_none());

        let status: String = sqlx::query_scalar!(
            "SELECT extraction_status FROM document_chunks WHERE id = $1",
            chunk_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "no_mention");
    }

    /// TC-REQ-003-4: zero rows persisted, chunk marked failed.
#[sqlx::test(migrations = "../web/migrations")]
    async fn extract_and_store_marks_failed_on_malformed_json(pool: PgPool) {
        let chunk_id = seed_chunk(&pool).await;
        let llm = FixedResponseProvider::new(MALFORMED_JSON);

        let mention_id = extract_and_store(&pool, chunk_id, "chunk text", &llm).await.unwrap();
        assert!(mention_id.is_none());

        let mention_count: i64 = sqlx::query_scalar!(
            "SELECT count(*) FROM project_mentions WHERE document_chunk_id = $1",
            chunk_id
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(mention_count, 0);

        let status: String = sqlx::query_scalar!(
            "SELECT extraction_status FROM document_chunks WHERE id = $1",
            chunk_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "failed");
    }

    /// TC-REQ-003-5 (retry-exhausted half): a sustained LLM failure marks
    /// the chunk reprocessing (retryable), not failed (permanent).
#[sqlx::test(migrations = "../web/migrations")]
    async fn extract_and_store_marks_reprocessing_on_llm_failure(pool: PgPool) {
        let chunk_id = seed_chunk(&pool).await;

        let mention_id = extract_and_store(&pool, chunk_id, "chunk text", &AlwaysFailingProvider)
            .await
            .unwrap();
        assert!(mention_id.is_none());

        let status: String = sqlx::query_scalar!(
            "SELECT extraction_status FROM document_chunks WHERE id = $1",
            chunk_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "reprocessing");
    }
}
