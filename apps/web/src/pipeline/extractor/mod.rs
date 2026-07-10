pub mod llm;
pub mod prompts;
pub mod schema;
pub mod scale;
pub mod validator;

use uuid::Uuid;

use crate::pipeline::normalizer;
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
pub async fn extract_entities(
    chunk_text: &str,
    llm: &dyn LlmProvider,
) -> Result<Option<ExtractionResult>, ExtractError> {
    let raw_json = llm
        .complete(prompts::en::SYSTEM_PROMPT, chunk_text)
        .await
        .map_err(|e| ExtractError::Llm(e.to_string()))?;

    let raw: RawExtraction = serde_json::from_str(&raw_json)
        .map_err(|e| ExtractError::MalformedJson(format!("{e}: {raw_json}")))?;

    if !raw.has_mention {
        return Ok(None);
    }

    let physical_work = validator::validate_physical_work(chunk_text, raw.physical_work);
    if !physical_work {
        return Ok(None);
    }

    if !scale::has_scale_indicator(raw.scale_units, raw.scale_gfa_sqm, raw.scale_storeys) {
        return Ok(None);
    }

    Ok(Some(ExtractionResult {
        physical_work,
        project_name: raw.project_name,
        civic_address: raw.civic_address,
        project_type: raw.project_type,
        scale_units: raw.scale_units,
        scale_gfa_sqm: raw.scale_gfa_sqm,
        scale_storeys: raw.scale_storeys,
        approval_status_raw: raw.approval_status_raw,
    }))
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
    let outcome = extract_entities(chunk_text, llm).await;

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
                  scale_units, scale_gfa_sqm, scale_storeys, approval_status_raw) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
                document_chunk_id,
                extraction.physical_work,
                extraction.project_name,
                extraction.civic_address,
                extraction.project_type,
                extraction.scale_units,
                extraction.scale_gfa_sqm,
                extraction.scale_storeys,
                extraction.approval_status_raw,
            )
            .fetch_one(pool)
            .await?;

            // REQ-004: normalize the raw status and flag any same-document
            // conflict, wired directly into the extraction output path
            // (IMP-REQ-004-06) rather than as a separate later pass.
            if let Some(raw_status) = &extraction.approval_status_raw {
                let language: Option<String> = sqlx::query_scalar!(
                    "SELECT language FROM document_chunks WHERE id = $1",
                    document_chunk_id
                )
                .fetch_one(pool)
                .await?;

                if let Some(language) = language {
                    if let Some(normalized) =
                        normalizer::normalize_status(pool, raw_status, &language).await?
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
        let result = extract_entities("Council approved 48-unit, 6-storey building at 123 Main St.", &llm)
            .await
            .unwrap();
        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(extraction.project_name.as_deref(), Some("Riverside Commons"));
        assert_eq!(extraction.scale_units, Some(48));
    }

    #[tokio::test]
    async fn extract_entities_returns_none_when_llm_reports_no_mention() {
        let llm = FixedResponseProvider::new(NO_MENTION_JSON);
        let result = extract_entities("The meeting was called to order.", &llm).await.unwrap();
        assert!(result.is_none());
    }

    /// TC-REQ-003-4: malformed LLM JSON discarded, not persisted.
    #[tokio::test]
    async fn extract_entities_returns_malformed_json_error() {
        let llm = FixedResponseProvider::new(MALFORMED_JSON);
        let result = extract_entities("Some chunk text.", &llm).await;
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

    #[sqlx::test(migrations = "./migrations")]
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

    #[sqlx::test(migrations = "./migrations")]
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
    #[sqlx::test(migrations = "./migrations")]
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
    #[sqlx::test(migrations = "./migrations")]
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
