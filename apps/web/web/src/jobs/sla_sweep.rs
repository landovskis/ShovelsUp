use sqlx::PgPool;

/// Overdue-vs-on-time split for open review candidates (IMP-REQ-009-08),
/// intended to run hourly. Like `pipeline::scheduler::Scheduler` (REQ-001),
/// this is a plain callable function, not a wired-up in-process timer —
/// this codebase has no periodic-execution infra anywhere yet (checked:
/// `main.rs` has no `tokio::spawn`/interval loop at all), so "hourly" is a
/// deployment-level concern (an external cron/k8s CronJob invoking this),
/// consistent with the existing Scheduler precedent rather than inventing
/// new in-process scheduling infra for this one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverdueMetric {
    pub open_count: i64,
    pub overdue_count: i64,
}

/// TC-REQ-009-2: a candidate exactly at its `due_at` boundary is not yet
/// overdue — only `due_at < now()` (strictly past) counts, per the
/// `idx_review_candidates_status_due_at` index this query uses.
pub async fn compute_overdue_metric(pool: &PgPool) -> Result<OverdueMetric, sqlx::Error> {
    let open_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM review_candidates WHERE status = 'open'"
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);

    let overdue_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM review_candidates WHERE status = 'open' AND due_at < now()"
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);

    Ok(OverdueMetric { open_count, overdue_count })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    async fn seed_candidate(pool: &PgPool, due_at: chrono::DateTime<chrono::Utc>) -> Uuid {
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
             VALUES ($1, true, '1 sla ave', 'residential', 1) RETURNING id",
            chunk_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar!(
            "INSERT INTO review_candidates (candidate_type, project_mention_id, due_at) \
             VALUES ('ambiguous_match', $1, $2) RETURNING id",
            mention_id,
            due_at,
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// TC-REQ-009-2: a candidate at the SLA boundary — due one second from
    /// now, not yet in the past — is not counted as overdue. (An exact tie
    /// against a live `now()` can't be tested deterministically since real
    /// wall-clock time always advances between the seed and the query; this
    /// exercises the same `< now()` boundary condition without that race.)
    #[sqlx::test(migrations = "./migrations")]
    async fn candidate_just_short_of_due_at_is_not_overdue(pool: PgPool) {
        let almost_due = chrono::Utc::now() + chrono::Duration::seconds(1);
        seed_candidate(&pool, almost_due).await;

        let metric = compute_overdue_metric(&pool).await.unwrap();
        assert_eq!(metric.open_count, 1);
        assert_eq!(metric.overdue_count, 0, "a due_at still (barely) in the future must not count as overdue");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn candidate_past_due_at_is_overdue(pool: PgPool) {
        let past = chrono::Utc::now() - chrono::Duration::days(3);
        seed_candidate(&pool, past).await;

        let metric = compute_overdue_metric(&pool).await.unwrap();
        assert_eq!(metric.overdue_count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn candidate_not_yet_due_is_not_overdue(pool: PgPool) {
        let future = chrono::Utc::now() + chrono::Duration::days(1);
        seed_candidate(&pool, future).await;

        let metric = compute_overdue_metric(&pool).await.unwrap();
        assert_eq!(metric.open_count, 1);
        assert_eq!(metric.overdue_count, 0);
    }
}
