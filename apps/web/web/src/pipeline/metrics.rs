use sqlx::PgPool;

/// Field-completeness reported separately per source language
/// (IMP-REQ-007-05), so an aggregate EN/FR average can't mask one language
/// silently underperforming the other. Mirrors the four-field completeness
/// metric used in `tests/pipeline_extraction.rs` (civic_address,
/// project_type, a scale indicator, approval_status_raw) — `project_name`
/// is excluded there and here because it is legitimately absent from many
/// real agenda items, not a model failure.
#[derive(Debug, Clone, PartialEq)]
pub struct LanguageCompleteness {
    pub language: String,
    pub mention_count: i64,
    pub average_completeness: f64,
}

/// Queries `project_mentions` joined to its source `document_chunks.language`
/// and reports average field-completeness per language. Mentions whose
/// chunk has no recorded language are excluded (there is nothing to group
/// them by), not silently folded into either bucket.
pub async fn field_completeness_by_language(
    pool: &PgPool,
) -> Result<Vec<LanguageCompleteness>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT
            dc.language AS "language!",
            count(*) AS "mention_count!",
            avg(
                (
                    (pm.civic_address IS NOT NULL)::int
                    + (pm.project_type IS NOT NULL)::int
                    + (pm.scale_units IS NOT NULL OR pm.scale_gfa_sqm IS NOT NULL OR pm.scale_storeys IS NOT NULL)::int
                    + (pm.approval_status_raw IS NOT NULL)::int
                )::float8 / 4.0
            ) AS "average_completeness!"
        FROM project_mentions pm
        JOIN document_chunks dc ON dc.id = pm.document_chunk_id
        WHERE dc.language IS NOT NULL
        GROUP BY dc.language
        ORDER BY dc.language
        "#
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| LanguageCompleteness {
            language: row.language,
            mention_count: row.mention_count,
            average_completeness: row.average_completeness,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn seed_chunk(pool: &PgPool, language: &str) -> Uuid {
        let suffix = Uuid::new_v4();
        let name = format!("Test City {suffix}");
        let slug = format!("test-city-{suffix}");
        let domain = format!("test-city-{suffix}.example");
        let municipality_id = sqlx::query_scalar!(
            "INSERT INTO municipalities (name, slug, domain_allowlist) \
             VALUES ($1, $2, ARRAY[$3]) RETURNING id",
            name,
            slug,
            domain,
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let source_url = format!("https://test-city-{suffix}.example/doc");
        let doc_id = sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, $2, 'chk', ''::bytea, 'text/html') RETURNING id",
            municipality_id,
            source_url,
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar!(
            "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
             VALUES ($1, 0, 'chunk text', $2) RETURNING id",
            doc_id,
            language,
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn seed_mention(
        pool: &PgPool,
        chunk_id: Uuid,
        civic_address: Option<&str>,
        approval_status_raw: Option<&str>,
    ) {
        sqlx::query!(
            "INSERT INTO project_mentions \
             (document_chunk_id, physical_work, civic_address, project_type, scale_units, approval_status_raw) \
             VALUES ($1, true, $2, 'residential', 1, $3)",
            chunk_id,
            civic_address,
            approval_status_raw,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// IMP-REQ-007-05: EN and FR completeness reported as separate rows,
    /// not blended into one average.
    #[sqlx::test(migrations = "./migrations")]
    async fn reports_en_and_fr_completeness_separately(pool: PgPool) {
        let en_chunk = seed_chunk(&pool, "en").await;
        seed_mention(&pool, en_chunk, Some("123 Main St"), Some("Approved")).await; // 4/4
        seed_mention(&pool, en_chunk, Some("456 Oak Ave"), None).await; // 3/4

        let fr_chunk = seed_chunk(&pool, "fr").await;
        seed_mention(&pool, fr_chunk, None, Some("Approuvé")).await; // 3/4

        let results = field_completeness_by_language(&pool).await.unwrap();
        assert_eq!(results.len(), 2);

        let en = results.iter().find(|r| r.language == "en").unwrap();
        assert_eq!(en.mention_count, 2);
        assert!((en.average_completeness - 0.875).abs() < 1e-9);

        let fr = results.iter().find(|r| r.language == "fr").unwrap();
        assert_eq!(fr.mention_count, 1);
        assert!((fr.average_completeness - 0.75).abs() < 1e-9);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn excludes_mentions_with_no_recorded_chunk_language(pool: PgPool) {
        let chunk = seed_chunk(&pool, "en").await;
        // Clear the language back to NULL to exercise the exclusion path.
        sqlx::query!("UPDATE document_chunks SET language = NULL WHERE id = $1", chunk)
            .execute(&pool)
            .await
            .unwrap();
        seed_mention(&pool, chunk, Some("1 No Language Way"), Some("Approved")).await;

        let results = field_completeness_by_language(&pool).await.unwrap();
        assert!(results.is_empty());
    }
}
