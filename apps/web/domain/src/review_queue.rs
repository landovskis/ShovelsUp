use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ReviewQueueError {
    #[error("review candidate {0} not found")]
    NotFound(Uuid),
    #[error("review candidate {0} is not open (already confirmed or rejected)")]
    NotOpen(Uuid),
    #[error("stale version: candidate has moved on since it was last read")]
    VersionConflict,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Confirms an ambiguous `review_candidates` row (IMP-REQ-009-03),
/// resolving it to `project_id` and linking the underlying mention —
/// mirrors the linking side effects of `resolver::link_mention` (updates
/// `project_mentions.project_id` and appends a `project_timeline_events`
/// row) so a confirmed candidate immediately shows up on the project's
/// timeline (TC-REQ-009-1).
///
/// `expected_version` implements optimistic concurrency: the conditional
/// `UPDATE ... WHERE version = $expected_version` either succeeds or
/// affects zero rows, and a mismatch is detected *before* any other write
/// happens (TC-REQ-009-3) — checked by first confirming the row exists and
/// is still `open`, distinguishing "not found" from "stale version" so
/// callers get an accurate error.
pub async fn confirm_candidate(
    pool: &PgPool,
    candidate_id: Uuid,
    expected_version: i32,
    project_id: Uuid,
    actor: &str,
) -> Result<(), ReviewQueueError> {
    resolve_candidate(pool, candidate_id, expected_version, actor, "confirm", Some(project_id)).await
}

/// Rejects an ambiguous `review_candidates` row (IMP-REQ-009-03) — the
/// underlying mention is left unresolved (no project link created).
pub async fn reject_candidate(
    pool: &PgPool,
    candidate_id: Uuid,
    expected_version: i32,
    actor: &str,
) -> Result<(), ReviewQueueError> {
    resolve_candidate(pool, candidate_id, expected_version, actor, "reject", None).await
}

async fn resolve_candidate(
    pool: &PgPool,
    candidate_id: Uuid,
    expected_version: i32,
    actor: &str,
    action: &str,
    project_id: Option<Uuid>,
) -> Result<(), ReviewQueueError> {
    let mut tx = pool.begin().await?;

    let current = sqlx::query!(
        "SELECT status, version, project_mention_id FROM review_candidates WHERE id = $1 FOR UPDATE",
        candidate_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ReviewQueueError::NotFound(candidate_id))?;

    if current.status != "open" {
        return Err(ReviewQueueError::NotOpen(candidate_id));
    }
    if current.version != expected_version {
        return Err(ReviewQueueError::VersionConflict);
    }

    let new_status = match action {
        "confirm" => "confirmed",
        _ => "rejected",
    };

    sqlx::query!(
        "UPDATE review_candidates \
         SET status = $1, version = version + 1, resolved_project_id = $2 \
         WHERE id = $3",
        new_status,
        project_id,
        candidate_id,
    )
    .execute(&mut *tx)
    .await?;

    if let (Some(project_id), Some(mention_id)) = (project_id, current.project_mention_id) {
        sqlx::query!(
            "UPDATE project_mentions SET project_id = $1 WHERE id = $2",
            project_id,
            mention_id
        )
        .execute(&mut *tx)
        .await?;

        let normalized_status: Option<String> = sqlx::query_scalar!(
            "SELECT normalized_status FROM project_mentions WHERE id = $1",
            mention_id
        )
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query!(
            "INSERT INTO project_timeline_events (project_id, project_mention_id, normalized_status) \
             VALUES ($1, $2, $3)",
            project_id,
            mention_id,
            normalized_status
        )
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query!(
        "INSERT INTO audit_events (review_candidate_id, action, actor) VALUES ($1, $2, $3)",
        candidate_id,
        action,
        actor,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_open_candidate(pool: &PgPool) -> (Uuid, Uuid) {
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
        let chunk_id = sqlx::query_scalar!(
            "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
             VALUES ($1, 0, 'chunk text') RETURNING id",
            doc_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let mention_id = sqlx::query_scalar!(
            "INSERT INTO project_mentions \
             (document_chunk_id, physical_work, civic_address, project_type, scale_units) \
             VALUES ($1, true, '1 ambiguous ave', 'residential', 1) RETURNING id",
            chunk_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let candidate_id = sqlx::query_scalar!(
            "INSERT INTO review_candidates (candidate_type, project_mention_id) \
             VALUES ('ambiguous_match', $1) RETURNING id",
            mention_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        (candidate_id, mention_id)
    }

    async fn seed_project(pool: &PgPool) -> Uuid {
        sqlx::query_scalar!(
            "INSERT INTO projects (civic_address_normalized, project_type) \
             VALUES ('1 ambiguous avenue', 'industrial') RETURNING id"
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// TC-REQ-009-1: confirm merges the ambiguous candidate into the
    /// proposed project — the mention links, and a timeline event appears.
    #[sqlx::test(migrations = "../web/migrations")]
    async fn confirm_links_mention_and_creates_timeline_event(pool: PgPool) {
        let (candidate_id, mention_id) = seed_open_candidate(&pool).await;
        let project_id = seed_project(&pool).await;

        confirm_candidate(&pool, candidate_id, 1, project_id, "founder@example.com")
            .await
            .unwrap();

        let status: String =
            sqlx::query_scalar!("SELECT status FROM review_candidates WHERE id = $1", candidate_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "confirmed");

        let linked_project: Option<Uuid> =
            sqlx::query_scalar!("SELECT project_id FROM project_mentions WHERE id = $1", mention_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(linked_project, Some(project_id));

        let timeline_count: i64 = sqlx::query_scalar!(
            "SELECT count(*) FROM project_timeline_events WHERE project_mention_id = $1",
            mention_id
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(timeline_count, 1);
    }

    /// TC-REQ-009-3: stale version on confirm returns a typed conflict, no
    /// changes are made.
    #[sqlx::test(migrations = "../web/migrations")]
    async fn confirm_with_stale_version_returns_conflict_and_makes_no_changes(pool: PgPool) {
        let (candidate_id, _) = seed_open_candidate(&pool).await;
        let project_id = seed_project(&pool).await;

        let result = confirm_candidate(&pool, candidate_id, 999, project_id, "founder@example.com").await;
        assert!(matches!(result, Err(ReviewQueueError::VersionConflict)));

        let status: String =
            sqlx::query_scalar!("SELECT status FROM review_candidates WHERE id = $1", candidate_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "open", "no change must occur on a version conflict");
    }

    #[sqlx::test(migrations = "../web/migrations")]
    async fn confirm_missing_candidate_returns_not_found(pool: PgPool) {
        let result = confirm_candidate(&pool, Uuid::new_v4(), 1, Uuid::new_v4(), "founder@example.com").await;
        assert!(matches!(result, Err(ReviewQueueError::NotFound(_))));
    }

    #[sqlx::test(migrations = "../web/migrations")]
    async fn reject_marks_candidate_rejected_without_linking(pool: PgPool) {
        let (candidate_id, mention_id) = seed_open_candidate(&pool).await;

        reject_candidate(&pool, candidate_id, 1, "founder@example.com").await.unwrap();

        let status: String =
            sqlx::query_scalar!("SELECT status FROM review_candidates WHERE id = $1", candidate_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "rejected");

        let linked_project: Option<Uuid> =
            sqlx::query_scalar!("SELECT project_id FROM project_mentions WHERE id = $1", mention_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(linked_project, None);
    }

    #[sqlx::test(migrations = "../web/migrations")]
    async fn confirm_already_resolved_candidate_returns_not_open(pool: PgPool) {
        let (candidate_id, _) = seed_open_candidate(&pool).await;
        let project_id = seed_project(&pool).await;
        confirm_candidate(&pool, candidate_id, 1, project_id, "founder@example.com")
            .await
            .unwrap();

        let result = confirm_candidate(&pool, candidate_id, 2, project_id, "founder@example.com").await;
        assert!(matches!(result, Err(ReviewQueueError::NotOpen(_))));
    }
}
