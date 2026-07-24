use sqlx::PgPool;

/// Refreshes `public_search_documents` from `projects` (IMP-REQ-008-02).
///
/// **Interim scope decision (flagged, not silently assumed):** the plan's
/// Acceptance Criteria says the refresh "excludes `review_state=pending`",
/// but no requirement through REQ-007 has added a `review_state` column to
/// `projects` — that's REQ-009's confirm/reject workflow, which hasn't
/// shipped yet in this execution order. Under the current resolver
/// (REQ-005), a row only ever lands in `projects` via an unambiguous
/// `NewProject`/`Linked` outcome; a genuinely ambiguous match is flagged
/// into `review_candidates` and never gets a `projects` row at all. So
/// every current `projects` row is already "confirmed" by construction —
/// this refresh selects all of them. Once REQ-009 introduces a
/// `review_state` column, this query will need a `WHERE review_state =
/// 'confirmed'` clause; noted here so that isn't missed.
///
/// Each project's `municipality_name` and `normalized_status` are taken
/// from its most recently created mention — a project can only exist once
/// at least one mention resolved to it, and the latest mention is the best
/// available signal for "current" status.
///
/// `latest_meeting_date` (IMP-REQ-009-02, migration 024) is
/// `MAX(project_timeline_events.event_date)` across every timeline event
/// resolved to the project, via a second `LEFT JOIN LATERAL` independent of
/// the `latest` mention lateral above (a project's most-recent mention and
/// its most-recent timeline event are not necessarily the same row) — `LEFT`
/// so a project with no timeline events yet still gets a row, with
/// `latest_meeting_date` left `NULL` (TC-009-3: `sort=date` sorts those
/// last, `NULLS LAST`, rather than excluding them).
///
/// `source_count` (IMP-REQ-015-01/-04, migration 027) is
/// `COUNT(DISTINCT document_chunks.source_document_id)` across every
/// `project_mentions` row resolved to the project, via a third
/// `LEFT JOIN LATERAL` — `document_chunks.source_document_id` is the
/// cleanest available "distinct council source" identifier (a fallback to
/// `COUNT(DISTINCT document_chunk_id)` is only needed if that column didn't
/// exist, which it does since migration 001). `LEFT` so a project with no
/// mentions somehow left dangling still gets a row, with `source_count`
/// left `NULL` rather than `0` (the indicator, IMP-REQ-015-06/-07, treats
/// "unknown" and "zero" the same way: omit the sentence entirely, TC-015-5).
///
/// `first_detected_at` (IMP-REQ-015-02, migration 027) is set to `now()`
/// only on first INSERT and — exactly like `first_surfaced_at`
/// (IMP-REQ-004-02) — deliberately absent from the `ON CONFLICT DO UPDATE
/// SET` list below, so it is frozen at first-detection time forever after,
/// unaffected by any later refresh. See migration 027's own doc comment for
/// why this is a distinct column from `first_surfaced_at` rather than a
/// reuse of it under a second meaning.
pub async fn refresh_public_search_index(pool: &PgPool) -> Result<u64, sqlx::Error> {
    // IMP-REQ-003-04: `source_language` (migration 018) needs the same
    // ongoing maintenance on every insert/update this job already gives
    // `municipality_slug` (IMP-REQ-002-02) — the one-time migration backfill
    // only covered rows that existed at migration time, not new/updated
    // ones. Sourced from the same latest-mention's chunk already joined here
    // for `normalized_status`.
    //
    // IMP-REQ-004-02: `first_surfaced_at` (migration 019) is set to `now()`
    // only on first INSERT and is deliberately absent from the `ON CONFLICT
    // DO UPDATE SET` list below — Postgres never touches a column an
    // `ON CONFLICT ... DO UPDATE` doesn't name, so an existing row's value
    // is preserved forever across every subsequent refresh, regardless of
    // how many times other columns change.
    // IMP-REQ-008-05: `category_code` (migration 023) mirrors
    // `projects.category_code` the same way every other column here mirrors
    // its `projects`/mention source, so `run_search`'s `category` filter can
    // read it directly off `public_search_documents` without joining back to
    // `projects`.
    let result = sqlx::query!(
        r#"
        INSERT INTO public_search_documents
            (project_id, civic_address_normalized, municipality_name, municipality_slug, project_type, normalized_status, source_language, category_code, latest_meeting_date, source_count, first_surfaced_at, first_detected_at, updated_at)
        SELECT
            p.id,
            p.civic_address_normalized,
            m.name,
            m.slug,
            p.project_type,
            latest.normalized_status,
            latest.language,
            p.category_code,
            timeline.latest_meeting_date,
            sources.source_count,
            now(),
            now(),
            now()
        FROM projects p
        LEFT JOIN LATERAL (
            SELECT pm.normalized_status, dc.source_document_id, dc.language
            FROM project_mentions pm
            JOIN document_chunks dc ON dc.id = pm.document_chunk_id
            WHERE pm.project_id = p.id
            ORDER BY pm.created_at DESC
            LIMIT 1
        ) latest ON true
        LEFT JOIN source_documents sd ON sd.id = latest.source_document_id
        LEFT JOIN municipalities m ON m.id = sd.municipality_id
        LEFT JOIN LATERAL (
            SELECT MAX(pte.event_date) AS latest_meeting_date
            FROM project_timeline_events pte
            WHERE pte.project_id = p.id
        ) timeline ON true
        LEFT JOIN LATERAL (
            SELECT COUNT(DISTINCT dc.source_document_id) AS source_count
            FROM project_mentions pm
            JOIN document_chunks dc ON dc.id = pm.document_chunk_id
            WHERE pm.project_id = p.id
        ) sources ON true
        WHERE p.civic_address_normalized IS NOT NULL
        ON CONFLICT (project_id) DO UPDATE SET
            civic_address_normalized = EXCLUDED.civic_address_normalized,
            municipality_name = EXCLUDED.municipality_name,
            municipality_slug = EXCLUDED.municipality_slug,
            project_type = EXCLUDED.project_type,
            normalized_status = EXCLUDED.normalized_status,
            source_language = EXCLUDED.source_language,
            category_code = EXCLUDED.category_code,
            latest_meeting_date = EXCLUDED.latest_meeting_date,
            source_count = EXCLUDED.source_count,
            updated_at = now()
        "#
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    async fn seed_project_with_mention(
        pool: &PgPool,
        civic_address_normalized: &str,
        project_type: &str,
        municipality_name: &str,
        normalized_status: Option<&str>,
    ) -> Uuid {
        let project_id = sqlx::query_scalar!(
            "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, $2) RETURNING id",
            civic_address_normalized,
            project_type,
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let municipality_id = sqlx::query_scalar!(
            "INSERT INTO municipalities (name, slug, domain_allowlist) VALUES ($1, $2, ARRAY[$3]) RETURNING id",
            municipality_name,
            municipality_name.to_lowercase().replace(' ', "-"),
            format!("{}.example", municipality_name.to_lowercase().replace(' ', "-")),
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let doc_id = sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, $2, 'chk', ''::bytea, 'text/html') RETURNING id",
            municipality_id,
            format!("https://{}.example/doc", municipality_name.to_lowercase().replace(' ', "-")),
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let chunk_id = sqlx::query_scalar!(
            "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
             VALUES ($1, 0, 'chunk text', 'en') RETURNING id",
            doc_id
        )
        .fetch_one(pool)
        .await
        .unwrap();

        sqlx::query!(
            "INSERT INTO project_mentions \
             (document_chunk_id, project_id, physical_work, civic_address, project_type, scale_units, normalized_status) \
             VALUES ($1, $2, true, $3, $4, 1, $5)",
            chunk_id,
            project_id,
            civic_address_normalized,
            project_type,
            normalized_status,
        )
        .execute(pool)
        .await
        .unwrap();

        project_id
    }

    /// Like `seed_project_with_mention`, but also returns the seeded
    /// `project_mentions.id` (IMP-REQ-009-03), which
    /// `project_timeline_events.project_mention_id` requires — the base
    /// helper discards it since no test before this one needed it.
    async fn seed_project_with_mention_and_id(
        pool: &PgPool,
        civic_address_normalized: &str,
        project_type: &str,
        municipality_name: &str,
    ) -> (Uuid, Uuid) {
        let project_id = sqlx::query_scalar!(
            "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, $2) RETURNING id",
            civic_address_normalized,
            project_type,
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let municipality_id = sqlx::query_scalar!(
            "INSERT INTO municipalities (name, slug, domain_allowlist) VALUES ($1, $2, ARRAY[$3]) RETURNING id",
            municipality_name,
            municipality_name.to_lowercase().replace(' ', "-"),
            format!("{}.example", municipality_name.to_lowercase().replace(' ', "-")),
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let doc_id = sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, $2, 'chk', ''::bytea, 'text/html') RETURNING id",
            municipality_id,
            format!("https://{}.example/doc", municipality_name.to_lowercase().replace(' ', "-")),
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let chunk_id = sqlx::query_scalar!(
            "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
             VALUES ($1, 0, 'chunk text', 'en') RETURNING id",
            doc_id
        )
        .fetch_one(pool)
        .await
        .unwrap();

        let mention_id = sqlx::query_scalar!(
            "INSERT INTO project_mentions \
             (document_chunk_id, project_id, physical_work, civic_address, project_type, scale_units, normalized_status) \
             VALUES ($1, $2, true, $3, $4, 1, 'approved') RETURNING id",
            chunk_id,
            project_id,
            civic_address_normalized,
            project_type,
        )
        .fetch_one(pool)
        .await
        .unwrap();

        (project_id, mention_id)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn populates_index_from_projects_and_their_latest_mention(pool: PgPool) {
        let project_id = seed_project_with_mention(
            &pool,
            "123 main street",
            "residential",
            "Test City",
            Some("approved"),
        )
        .await;

        let affected = refresh_public_search_index(&pool).await.unwrap();
        assert_eq!(affected, 1);

        let row = sqlx::query!(
            "SELECT municipality_name, municipality_slug, project_type, normalized_status, source_language \
             FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.municipality_name.as_deref(), Some("Test City"));
        assert_eq!(row.municipality_slug.as_deref(), Some("test-city"));
        assert_eq!(row.project_type.as_deref(), Some("residential"));
        assert_eq!(row.normalized_status.as_deref(), Some("approved"));
        assert_eq!(
            row.source_language.as_deref(),
            Some("en"),
            "source_language must be set correctly on the insert path (IMP-REQ-003-04)"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn refresh_is_idempotent_and_updates_existing_rows(pool: PgPool) {
        let project_id = seed_project_with_mention(
            &pool,
            "456 oak avenue",
            "commercial",
            "Other City",
            Some("proposed"),
        )
        .await;

        refresh_public_search_index(&pool).await.unwrap();

        sqlx::query!(
            "UPDATE project_mentions SET normalized_status = 'approved' WHERE project_id = $1",
            project_id
        )
        .execute(&pool)
        .await
        .unwrap();

        refresh_public_search_index(&pool).await.unwrap();

        let count: i64 = sqlx::query_scalar!(
            "SELECT count(*) FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(count, 1, "must not create a duplicate row on re-run");

        let row = sqlx::query!(
            "SELECT normalized_status, municipality_slug, source_language FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.normalized_status.as_deref(),
            Some("approved"),
            "must reflect the latest mention status"
        );
        assert_eq!(
            row.source_language.as_deref(),
            Some("en"),
            "source_language must be set correctly on the upsert (update) path too"
        );
        assert_eq!(
            row.municipality_slug.as_deref(),
            Some("other-city"),
            "municipality_slug must be set correctly on the upsert (update) path too"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn first_surfaced_at_is_unchanged_across_repeated_refreshes(pool: PgPool) {
        let project_id = seed_project_with_mention(
            &pool,
            "789 pine boulevard",
            "residential",
            "Third City",
            Some("proposed"),
        )
        .await;

        refresh_public_search_index(&pool).await.unwrap();

        let first_surfaced_at_initial: chrono::DateTime<chrono::Utc> = sqlx::query_scalar!(
            "SELECT first_surfaced_at AS \"first_surfaced_at!\" FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query!(
            "UPDATE project_mentions SET normalized_status = 'approved' WHERE project_id = $1",
            project_id
        )
        .execute(&pool)
        .await
        .unwrap();

        refresh_public_search_index(&pool).await.unwrap();

        let first_surfaced_at_after_second_refresh: chrono::DateTime<chrono::Utc> = sqlx::query_scalar!(
            "SELECT first_surfaced_at AS \"first_surfaced_at!\" FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            first_surfaced_at_initial, first_surfaced_at_after_second_refresh,
            "first_surfaced_at must not change across a second refresh-job upsert of the same project"
        );
    }

    /// IMP-REQ-009-03: a project with a single timeline event gets that
    /// event's `event_date` mirrored onto `latest_meeting_date`.
    #[sqlx::test(migrations = "./migrations")]
    async fn latest_meeting_date_mirrors_single_timeline_event(pool: PgPool) {
        let (project_id, mention_id) = seed_project_with_mention_and_id(
            &pool,
            "1 rue timeline unique",
            "residential",
            "Ville Timeline Un",
        )
        .await;

        let event_date = chrono::Utc::now() - chrono::Duration::days(3);
        sqlx::query!(
            "INSERT INTO project_timeline_events (project_id, project_mention_id, event_date) VALUES ($1, $2, $3)",
            project_id,
            mention_id,
            event_date,
        )
        .execute(&pool)
        .await
        .unwrap();

        refresh_public_search_index(&pool).await.unwrap();

        let latest_meeting_date: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(
            "SELECT latest_meeting_date FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            latest_meeting_date, Some(event_date),
            "latest_meeting_date must mirror the project's single timeline event's event_date"
        );
    }

    /// IMP-REQ-009-03: with multiple timeline events, `latest_meeting_date`
    /// takes the MAX `event_date`, not the first/last inserted.
    #[sqlx::test(migrations = "./migrations")]
    async fn latest_meeting_date_takes_max_event_date_across_multiple_events(pool: PgPool) {
        let (project_id, mention_id) = seed_project_with_mention_and_id(
            &pool,
            "2 rue timeline multiple",
            "commercial",
            "Ville Timeline Deux",
        )
        .await;

        let earlier = chrono::Utc::now() - chrono::Duration::days(30);
        let latest = chrono::Utc::now() - chrono::Duration::days(1);
        let middle = chrono::Utc::now() - chrono::Duration::days(10);

        for event_date in [earlier, latest, middle] {
            sqlx::query!(
                "INSERT INTO project_timeline_events (project_id, project_mention_id, event_date) VALUES ($1, $2, $3)",
                project_id,
                mention_id,
                event_date,
            )
            .execute(&pool)
            .await
            .unwrap();
        }

        refresh_public_search_index(&pool).await.unwrap();

        let latest_meeting_date: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(
            "SELECT latest_meeting_date FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            latest_meeting_date, Some(latest),
            "latest_meeting_date must be the MAX event_date across all of the project's timeline events"
        );
    }

    /// IMP-REQ-009-03 / TC-009-3: a project with NO timeline events gets a
    /// NULL `latest_meeting_date` (the `LEFT JOIN LATERAL` must not drop the
    /// project's row, nor invent a date).
    #[sqlx::test(migrations = "./migrations")]
    async fn latest_meeting_date_is_null_with_no_timeline_events(pool: PgPool) {
        let project_id = seed_project_with_mention(
            &pool,
            "3 rue sans timeline",
            "institutional",
            "Ville Timeline Trois",
            Some("proposed"),
        )
        .await;

        refresh_public_search_index(&pool).await.unwrap();

        let latest_meeting_date: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(
            "SELECT latest_meeting_date FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            latest_meeting_date, None,
            "a project with no timeline events must get a NULL latest_meeting_date, not a fabricated one"
        );
    }

    /// IMP-REQ-009-03: `latest_meeting_date` is re-derived (not frozen like
    /// `first_surfaced_at`) on a second refresh once a new, later timeline
    /// event is added — it must track the live MAX, unlike
    /// `first_surfaced_at`'s deliberate immutability.
    #[sqlx::test(migrations = "./migrations")]
    async fn latest_meeting_date_updates_on_subsequent_refresh(pool: PgPool) {
        let (project_id, mention_id) = seed_project_with_mention_and_id(
            &pool,
            "4 rue timeline evolutive",
            "infrastructure",
            "Ville Timeline Quatre",
        )
        .await;

        let first_event_date = chrono::Utc::now() - chrono::Duration::days(20);
        sqlx::query!(
            "INSERT INTO project_timeline_events (project_id, project_mention_id, event_date) VALUES ($1, $2, $3)",
            project_id,
            mention_id,
            first_event_date,
        )
        .execute(&pool)
        .await
        .unwrap();

        refresh_public_search_index(&pool).await.unwrap();

        let after_first_refresh: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(
            "SELECT latest_meeting_date FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after_first_refresh, Some(first_event_date));

        let later_event_date = chrono::Utc::now() - chrono::Duration::days(2);
        sqlx::query!(
            "INSERT INTO project_timeline_events (project_id, project_mention_id, event_date) VALUES ($1, $2, $3)",
            project_id,
            mention_id,
            later_event_date,
        )
        .execute(&pool)
        .await
        .unwrap();

        refresh_public_search_index(&pool).await.unwrap();

        let after_second_refresh: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(
            "SELECT latest_meeting_date FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            after_second_refresh, Some(later_event_date),
            "latest_meeting_date must be re-derived on each refresh to track the live MAX(event_date), not frozen like first_surfaced_at"
        );
    }
}
