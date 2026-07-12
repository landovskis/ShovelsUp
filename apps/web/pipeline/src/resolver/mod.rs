pub(crate) mod address;
pub(crate) mod address_fr;
pub(crate) mod core;

use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("mention {0} not found")]
    MentionNotFound(Uuid),
    #[error("database error after retries: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionOutcome {
    /// Linked to an existing project via an explicit cross-reference or a
    /// single address+type match.
    Linked { project_id: Uuid },
    /// No existing project matched; a new one was created and linked.
    NewProject { project_id: Uuid },
    /// Multiple projects matched on address+type — ambiguous; flagged into
    /// `review_candidates`, not auto-linked (TC-REQ-005-4).
    FlaggedAmbiguous { review_candidate_id: Uuid },
    /// The mention lacks enough information (no civic address, or no
    /// project type) to attempt resolution at all.
    InsufficientData,
}

const MAX_ATTEMPTS: u32 = 5;

/// Resolves `mention_id` into a tracked `projects` record, in priority
/// order: explicit cross-reference first, then address+type match (RULE-003).
/// Wraps the whole operation in retry/backoff for transient DB failures
/// (IMP-REQ-005-08) — connection-class errors are retried; a unique-index
/// race on `projects(civic_address_normalized, project_type)` (two
/// concurrent resolutions of a genuinely new address) is handled inline as
/// an "insert-or-get" outcome, not a failure (TC-REQ-005-7 concurrency).
pub async fn resolve_mention(
    pool: &PgPool,
    mention_id: Uuid,
) -> Result<ResolutionOutcome, ResolveError> {
    match retry_transient(|| try_resolve(pool, mention_id)).await {
        Ok(outcome) => Ok(outcome),
        Err(sqlx::Error::RowNotFound) => Err(ResolveError::MentionNotFound(mention_id)),
        Err(err) => Err(err.into()),
    }
}

/// Retries `attempt_fn` up to `MAX_ATTEMPTS` times, sleeping `backoff_delay`
/// between attempts, as long as the returned error is transient
/// (TC-REQ-005-5). A non-transient error (e.g. `RowNotFound`, a unique
/// violation) returns immediately without retrying. Extracted as a generic
/// helper so the retry/backoff behaviour itself is unit-testable without a
/// real database (see `retry_tests` below).
async fn retry_transient<T, F, Fut>(mut attempt_fn: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match attempt_fn().await {
            Ok(value) => return Ok(value),
            Err(err) if is_transient(&err) && attempt < MAX_ATTEMPTS => {
                tokio::time::sleep(backoff_delay(attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn try_resolve(pool: &PgPool, mention_id: Uuid) -> Result<ResolutionOutcome, sqlx::Error> {
    // IMP-REQ-007-02 wiring: the source chunk's language picks the address
    // normalizer, mirroring extract_and_store's language routing, so a
    // French mention's civic address is normalized with the Quebec ruleset
    // rather than the English one (address.rs's own docstring says this
    // module's matcher logic is shared but normalization differs by
    // language — this is the one place that dispatch has to happen).
    let mention = sqlx::query!(
        "SELECT pm.civic_address, pm.project_type, pm.reference_number, dc.language \
         FROM project_mentions pm \
         JOIN document_chunks dc ON dc.id = pm.document_chunk_id \
         WHERE pm.id = $1",
        mention_id
    )
    .fetch_optional(pool)
    .await?
    .ok_or(sqlx::Error::RowNotFound)?;

    // Priority 1: explicit cross-reference.
    if let Some(reference_number) = &mention.reference_number {
        if let Some(project_id) = cross_ref_match(pool, reference_number, mention_id).await? {
            link_mention(pool, mention_id, project_id).await?;
            return Ok(ResolutionOutcome::Linked { project_id });
        }
    }

    let (Some(civic_address), Some(project_type)) = (&mention.civic_address, &mention.project_type)
    else {
        return Ok(ResolutionOutcome::InsufficientData);
    };
    let normalized_address = match mention.language.as_deref() {
        Some("fr") => address_fr::normalize_address_fr(civic_address),
        _ => address::normalize_address(civic_address),
    };

    // Priority 2: address+type match. Exact (address, type) match is
    // unambiguous and links directly. The partial unique index on
    // projects(civic_address_normalized, project_type) means this exact
    // query can never itself return >1 row — genuine ambiguity instead
    // comes from the SAME address already having project(s) of a
    // *different* type (e.g. a site redeveloped from industrial to
    // residential): we can't tell whether the new mention continues one of
    // those under a corrected type or is a genuinely new project at that
    // address, so that case is flagged for human review rather than
    // guessed (RULE-003: ambiguous matches are not auto-merged or
    // auto-split).
    let exact_match = sqlx::query_scalar!(
        "SELECT id FROM projects WHERE civic_address_normalized = $1 AND project_type = $2",
        normalized_address,
        project_type
    )
    .fetch_optional(pool)
    .await?;

    let other_type_matches = sqlx::query_scalar!(
        "SELECT id FROM projects WHERE civic_address_normalized = $1 AND project_type != $2",
        normalized_address,
        project_type
    )
    .fetch_all(pool)
    .await?;

    match core::decide_address_resolution(exact_match, other_type_matches.len()) {
        core::AddressResolutionDecision::Link { project_id } => {
            link_mention(pool, mention_id, project_id).await?;
            Ok(ResolutionOutcome::Linked { project_id })
        }
        core::AddressResolutionDecision::CreateProject => {
            // No match of any type at this address: create a new project.
            // A concurrent resolution of the same new (address, type) races
            // here — the partial unique index rejects the loser, which
            // re-queries and links to the winner's project instead of
            // erroring (TC-REQ-005-7).
            match sqlx::query_scalar!(
                "INSERT INTO projects (civic_address_normalized, project_type) \
                 VALUES ($1, $2) RETURNING id",
                normalized_address,
                project_type
            )
            .fetch_one(pool)
            .await
            {
                Ok(project_id) => {
                    link_mention(pool, mention_id, project_id).await?;
                    Ok(ResolutionOutcome::NewProject { project_id })
                }
                Err(err) if is_unique_violation(&err) => {
                    let project_id = sqlx::query_scalar!(
                        "SELECT id FROM projects WHERE civic_address_normalized = $1 AND project_type = $2",
                        normalized_address,
                        project_type
                    )
                    .fetch_one(pool)
                    .await?;
                    link_mention(pool, mention_id, project_id).await?;
                    Ok(ResolutionOutcome::Linked { project_id })
                }
                Err(err) => Err(err),
            }
        }
        core::AddressResolutionDecision::FlagAmbiguous => {
            let details = serde_json::json!({
                "civic_address": civic_address,
                "normalized_address": normalized_address,
                "project_type": project_type,
                "candidate_project_ids": other_type_matches,
            });
            let review_candidate_id = sqlx::query_scalar!(
                "INSERT INTO review_candidates (candidate_type, project_mention_id, details) \
                 VALUES ('ambiguous_match', $1, $2) RETURNING id",
                mention_id,
                details
            )
            .fetch_one(pool)
            .await?;
            Ok(ResolutionOutcome::FlaggedAmbiguous {
                review_candidate_id,
            })
        }
    }
}

async fn cross_ref_match(
    pool: &PgPool,
    reference_number: &str,
    excluding_mention_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT project_id FROM project_mentions \
         WHERE reference_number = $1 AND project_id IS NOT NULL AND id != $2 \
         LIMIT 1",
        reference_number,
        excluding_mention_id
    )
    .fetch_optional(pool)
    .await
    .map(|opt| opt.flatten())
}

async fn link_mention(
    pool: &PgPool,
    mention_id: Uuid,
    project_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE project_mentions SET project_id = $1 WHERE id = $2",
        project_id,
        mention_id
    )
    .execute(pool)
    .await?;

    let normalized_status: Option<String> = sqlx::query_scalar!(
        "SELECT normalized_status FROM project_mentions WHERE id = $1",
        mention_id
    )
    .fetch_one(pool)
    .await?;

    sqlx::query!(
        "INSERT INTO project_timeline_events (project_id, project_mention_id, normalized_status) \
         VALUES ($1, $2, $3)",
        project_id,
        mention_id,
        normalized_status
    )
    .execute(pool)
    .await?;

    Ok(())
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("23505"))
}

fn is_transient(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Io(_)
            | sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::WorkerCrashed
    ) || matches!(
        err,
        sqlx::Error::Database(db_err)
            if db_err.code().is_some_and(|c| c.starts_with("08") || c.starts_with("53") || c.starts_with("40"))
    )
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(50u64 * 2u64.pow(attempt))
}

/// TC-REQ-005-5: DB unavailability during resolution is retryable, not
/// dropped. `retry_transient` is exercised directly against injected
/// `sqlx::Error` values (no real Postgres outage needed) with tokio's
/// virtual clock so the backoff delays don't slow the test suite down.
#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test(start_paused = true)]
    async fn retries_transient_failure_and_succeeds_on_third_attempt() {
        let attempts = AtomicU32::new(0);
        let result: Result<u32, sqlx::Error> = retry_transient(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if n < 3 {
                    Err(sqlx::Error::PoolClosed)
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "must succeed on the 3rd attempt"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn non_transient_error_is_not_retried() {
        let attempts = AtomicU32::new(0);
        let result: Result<(), sqlx::Error> = retry_transient(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(sqlx::Error::RowNotFound) }
        })
        .await;

        assert!(matches!(result, Err(sqlx::Error::RowNotFound)));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a non-transient error must not be retried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn exhausts_max_attempts_on_persistent_transient_failure() {
        let attempts = AtomicU32::new(0);
        let result: Result<(), sqlx::Error> = retry_transient(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(sqlx::Error::PoolClosed) }
        })
        .await;

        assert!(
            result.is_err(),
            "must surface the error once attempts are exhausted, not hang forever"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            MAX_ATTEMPTS,
            "no orphaned retries beyond MAX_ATTEMPTS"
        );
    }

    #[test]
    fn is_transient_classifies_connection_and_serialization_errors() {
        assert!(is_transient(&sqlx::Error::PoolClosed));
        assert!(is_transient(&sqlx::Error::PoolTimedOut));
        assert!(is_transient(&sqlx::Error::WorkerCrashed));
    }

    #[test]
    fn is_transient_does_not_retry_row_not_found_or_constraint_violations() {
        assert!(!is_transient(&sqlx::Error::RowNotFound));
    }
}
