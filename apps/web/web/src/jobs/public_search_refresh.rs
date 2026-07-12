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
pub async fn refresh_public_search_index(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        INSERT INTO public_search_documents
            (project_id, civic_address_normalized, municipality_name, project_type, normalized_status, updated_at)
        SELECT
            p.id,
            p.civic_address_normalized,
            m.name,
            p.project_type,
            latest.normalized_status,
            now()
        FROM projects p
        LEFT JOIN LATERAL (
            SELECT pm.normalized_status, dc.source_document_id
            FROM project_mentions pm
            JOIN document_chunks dc ON dc.id = pm.document_chunk_id
            WHERE pm.project_id = p.id
            ORDER BY pm.created_at DESC
            LIMIT 1
        ) latest ON true
        LEFT JOIN source_documents sd ON sd.id = latest.source_document_id
        LEFT JOIN municipalities m ON m.id = sd.municipality_id
        WHERE p.civic_address_normalized IS NOT NULL
        ON CONFLICT (project_id) DO UPDATE SET
            civic_address_normalized = EXCLUDED.civic_address_normalized,
            municipality_name = EXCLUDED.municipality_name,
            project_type = EXCLUDED.project_type,
            normalized_status = EXCLUDED.normalized_status,
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
            "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
             VALUES ($1, 0, 'chunk text') RETURNING id",
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
            "SELECT municipality_name, project_type, normalized_status \
             FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.municipality_name.as_deref(), Some("Test City"));
        assert_eq!(row.project_type.as_deref(), Some("residential"));
        assert_eq!(row.normalized_status.as_deref(), Some("approved"));
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

        let status: Option<String> = sqlx::query_scalar!(
            "SELECT normalized_status FROM public_search_documents WHERE project_id = $1",
            project_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            status.as_deref(),
            Some("approved"),
            "must reflect the latest mention status"
        );
    }
}
