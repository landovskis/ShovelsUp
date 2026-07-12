pub(crate) mod core;

use sqlx::PgPool;
use uuid::Uuid;

use crate::extractor::{extract_and_store, llm::LlmProvider};
use crate::fetcher::{FetchOutcome, Fetcher};
use crate::parser::{ocr::OcrProvider, orchestrate::parse_and_store};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct WorkerSummary {
    pub documents_ingested: usize,
    pub documents_skipped_duplicate: usize,
    pub failed: usize,
    pub skipped_no_agenda_url: usize,
}

/// Drives every due, pending `fetch_jobs` row through discovery → fetch →
/// parse → extract. See the design doc
/// (docs/superpowers/specs/2026-07-11-fetch-job-worker-design.md) for the
/// full per-job/per-document state machine.
pub async fn run_due_fetch_jobs(
    pool: &PgPool,
    ocr: &dyn OcrProvider,
    llm: &dyn LlmProvider,
) -> Result<WorkerSummary, sqlx::Error> {
    let mut summary = WorkerSummary::default();
    let fetcher = Fetcher::new();

    let due_jobs: Vec<(Uuid, Uuid)> = sqlx::query!(
        "SELECT id, municipality_id FROM fetch_jobs \
         WHERE status = 'pending' AND scheduled_for <= now()"
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| (row.id, row.municipality_id))
    .collect();

    for (job_id, municipality_id) in due_jobs {
        let agenda_url: Option<String> = sqlx::query_scalar!(
            "SELECT agenda_url FROM municipalities WHERE id = $1",
            municipality_id
        )
        .fetch_one(pool)
        .await?;

        let Some(agenda_url) = agenda_url else {
            summary.skipped_no_agenda_url += 1;
            continue;
        };

        sqlx::query!(
            "UPDATE fetch_jobs SET status = 'in_progress', updated_at = now() WHERE id = $1",
            job_id
        )
        .execute(pool)
        .await?;

        let html_bytes = match fetcher.fetch_bytes(pool, municipality_id, &agenda_url).await {
            Ok(bytes) => bytes,
            Err(err) => {
                sqlx::query!(
                    "UPDATE fetch_jobs SET status = 'failed', attempts = attempts + 1, \
                     last_error = $1, updated_at = now() WHERE id = $2",
                    err.to_string(),
                    job_id
                )
                .execute(pool)
                .await?;
                summary.failed += 1;
                continue;
            }
        };

        let html = String::from_utf8_lossy(&html_bytes).into_owned();
        let document_urls = core::extract_pv_document_links(&html, &agenda_url);

        for document_url in document_urls {
            // fetch_optional + is_some(), not `SELECT EXISTS(...)`: sqlx's
            // nullability inference for a computed EXISTS() expression is
            // not guaranteed to produce Option<bool> vs. plain bool, but a
            // plain column selection's Option-ness is reliably inferred —
            // same pattern as Scheduler::enqueue_due_fetches's
            // `already_scheduled_today` check.
            let already_ingested = sqlx::query_scalar!(
                "SELECT id FROM source_documents WHERE municipality_id = $1 AND source_url = $2",
                municipality_id,
                document_url
            )
            .fetch_optional(pool)
            .await?
            .is_some();

            if already_ingested {
                summary.documents_skipped_duplicate += 1;
                continue;
            }

            match fetcher.fetch(pool, municipality_id, &document_url).await {
                Err(err) => {
                    tracing::warn!(
                        job_id = %job_id,
                        url = %document_url,
                        error = %err,
                        "failed to fetch a discovered document"
                    );
                    summary.failed += 1;
                }
                Ok(FetchOutcome::Duplicate { .. }) => {
                    summary.documents_skipped_duplicate += 1;
                }
                Ok(FetchOutcome::Fetched { document_id }) => {
                    parse_and_store(pool, document_id, ocr).await?;

                    let chunks = sqlx::query!(
                        "SELECT id, content FROM document_chunks WHERE source_document_id = $1",
                        document_id
                    )
                    .fetch_all(pool)
                    .await?;

                    for chunk in chunks {
                        extract_and_store(pool, chunk.id, &chunk.content, llm).await?;
                    }

                    summary.documents_ingested += 1;
                }
            }
        }

        sqlx::query!(
            "UPDATE fetch_jobs SET status = 'succeeded', updated_at = now() WHERE id = $1",
            job_id
        )
        .execute(pool)
        .await?;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A municipality with no agenda_url configured (Toronto/Vancouver
    /// today) must be skipped, not treated as a failure — see the design
    /// doc's worker step 1.
    #[sqlx::test(migrations = "../web/migrations")]
    async fn run_due_fetch_jobs_skips_municipality_with_no_agenda_url(pool: PgPool) {
        // The 002 seed migration inserts Toronto/Vancouver with agenda_url
        // NULL and a pending fetch_jobs row is created for each via the
        // scheduler in a real run; here we insert one directly to isolate
        // this unit from Scheduler's behavior.
        let municipality_id: Uuid = sqlx::query_scalar!(
            "SELECT id FROM municipalities WHERE slug = 'toronto'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO fetch_jobs (municipality_id, scheduled_for) VALUES ($1, now())",
            municipality_id
        )
        .execute(&pool)
        .await
        .unwrap();

        let ocr = crate::parser::ocr::TesseractOcrProvider;
        let llm = crate::extractor::llm::AnthropicProvider::new("unused".to_string());
        let summary = run_due_fetch_jobs(&pool, &ocr, &llm).await.unwrap();

        assert_eq!(summary.skipped_no_agenda_url, 1);

        let status: String = sqlx::query_scalar!(
            "SELECT status FROM fetch_jobs WHERE municipality_id = $1",
            municipality_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "pending", "job with no agenda_url stays pending, not failed");
    }
}
