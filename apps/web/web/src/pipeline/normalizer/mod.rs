use sqlx::PgPool;
use uuid::Uuid;

/// Deterministic lookup — not an LLM call — against the seeded
/// `status_vocabulary` table, so EN/FR parity is a data-coverage problem
/// (matching synonyms) rather than a model-consistency one (per REQ-004's
/// Implementation Strategy). Picks the longest matching seed phrase that
/// appears anywhere in `raw_text` (case-insensitive), so phrasing like
/// "Deferred to next meeting." matches the seeded "deferred" phrase without
/// requiring an exact match against the whole sentence. Returns `None` — not
/// a default — when nothing matches (TC-REQ-004-3: an unrecognized phrase
/// must not be silently defaulted).
pub async fn normalize_status(
    pool: &PgPool,
    raw_text: &str,
    language: &str,
) -> Result<Option<String>, sqlx::Error> {
    let cleaned = raw_text.to_lowercase();

    let vocab = sqlx::query!(
        "SELECT phrase, normalized_status FROM status_vocabulary WHERE language = $1",
        language
    )
    .fetch_all(pool)
    .await?;

    let mut best: Option<(usize, String)> = None;
    for row in vocab {
        if cleaned.contains(&row.phrase) {
            let len = row.phrase.len();
            if best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                best = Some((len, row.normalized_status));
            }
        }
    }

    Ok(best.map(|(_, status)| status))
}

/// Detects a same-document status conflict for `new_mention_id`: another
/// mention in the same source document sharing the same civic address but
/// carrying a different normalized status. Creates a `review_candidates`
/// row (`candidate_type = 'status_conflict'`) recording both statuses.
///
/// LIMITATION (documented, not silently assumed): the PRD's "resolved to
/// the later, more specific dated event" implies per-mention event dates,
/// which don't exist yet in this schema (no per-agenda-item date is
/// captured anywhere before REQ-006's timeline). This uses insertion order
/// (`created_at`) as an interim proxy — the most-recently-extracted mention
/// is treated as authoritative — and flags the conflict for human
/// visibility either way, so a wrong auto-resolution is still reviewable.
///
/// Returns `Ok(None)` if there's nothing to compare (missing address/status
/// on the new mention, or no conflicting sibling).
pub async fn detect_and_flag_status_conflict(
    pool: &PgPool,
    new_mention_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mention = sqlx::query!(
        "SELECT pm.civic_address, pm.normalized_status, dc.source_document_id \
         FROM project_mentions pm \
         JOIN document_chunks dc ON dc.id = pm.document_chunk_id \
         WHERE pm.id = $1",
        new_mention_id
    )
    .fetch_one(pool)
    .await?;

    let (Some(civic_address), Some(normalized_status)) =
        (mention.civic_address, mention.normalized_status)
    else {
        return Ok(None);
    };

    let conflicting = sqlx::query!(
        "SELECT pm.id, pm.normalized_status as status \
         FROM project_mentions pm \
         JOIN document_chunks dc ON dc.id = pm.document_chunk_id \
         WHERE dc.source_document_id = $1 \
           AND pm.civic_address = $2 \
           AND pm.normalized_status IS NOT NULL \
           AND pm.normalized_status != $3 \
           AND pm.id != $4",
        mention.source_document_id,
        civic_address,
        normalized_status,
        new_mention_id
    )
    .fetch_all(pool)
    .await?;

    if conflicting.is_empty() {
        return Ok(None);
    }

    let details = serde_json::json!({
        "civic_address": civic_address,
        "resolved_status": normalized_status,
        "resolution_basis": "most_recently_extracted_mention",
        "conflicting_mention_ids": conflicting.iter().map(|c| c.id).collect::<Vec<_>>(),
        "conflicting_statuses": conflicting.iter().map(|c| c.status.clone()).collect::<Vec<_>>(),
    });

    let candidate_id = sqlx::query_scalar!(
        "INSERT INTO review_candidates (candidate_type, project_mention_id, details) \
         VALUES ('status_conflict', $1, $2) RETURNING id",
        new_mention_id,
        details
    )
    .fetch_one(pool)
    .await?;

    Ok(Some(candidate_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_document_chunk(pool: &PgPool) -> Uuid {
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
             VALUES ($1, 0, 'chunk text') RETURNING id",
            doc_id
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn insert_mention(
        pool: &PgPool,
        chunk_id: Uuid,
        civic_address: Option<&str>,
        normalized_status: Option<&str>,
    ) -> Uuid {
        sqlx::query_scalar!(
            "INSERT INTO project_mentions \
             (document_chunk_id, physical_work, civic_address, normalized_status, scale_units) \
             VALUES ($1, true, $2, $3, 1) RETURNING id",
            chunk_id,
            civic_address,
            normalized_status,
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// TC-REQ-004-1: English synonyms map to the correct enum value.
    #[sqlx::test(migrations = "./migrations")]
    async fn normalize_status_maps_english_synonyms(pool: PgPool) {
        assert_eq!(
            normalize_status(&pool, "Approved.", "en").await.unwrap(),
            Some("approved".to_string())
        );
        assert_eq!(
            normalize_status(&pool, "The item was adopted unanimously.", "en").await.unwrap(),
            Some("approved".to_string())
        );
        assert_eq!(
            normalize_status(&pool, "Deferred to next meeting.", "en").await.unwrap(),
            Some("deferred".to_string())
        );
        assert_eq!(
            normalize_status(&pool, "Referred to committee.", "en").await.unwrap(),
            Some("referred".to_string())
        );
        assert_eq!(
            normalize_status(&pool, "Rejected.", "en").await.unwrap(),
            Some("rejected".to_string())
        );
    }

    /// TC-REQ-004-2: French synonyms map to the same enum value as EN.
    #[sqlx::test(migrations = "./migrations")]
    async fn normalize_status_maps_french_synonyms_to_same_values(pool: PgPool) {
        assert_eq!(
            normalize_status(&pool, "Approuvé.", "fr").await.unwrap(),
            Some("approved".to_string())
        );
        assert_eq!(
            normalize_status(&pool, "Reporté à la prochaine séance.", "fr").await.unwrap(),
            Some("deferred".to_string())
        );
        assert_eq!(
            normalize_status(&pool, "Rejeté.", "fr").await.unwrap(),
            Some("rejected".to_string())
        );
    }

    /// TC-REQ-004-3: unrecognized phrase not silently defaulted.
    #[sqlx::test(migrations = "./migrations")]
    async fn normalize_status_returns_none_for_unrecognized_phrase(pool: PgPool) {
        let result = normalize_status(&pool, "Tabled for further study.", "en").await.unwrap();
        assert_eq!(result, None);
    }

    /// TC-REQ-004-4: conflicting same-document status resolved + flagged.
    #[sqlx::test(migrations = "./migrations")]
    async fn detects_and_flags_same_document_status_conflict(pool: PgPool) {
        let chunk_id = seed_document_chunk(&pool).await;
        insert_mention(&pool, chunk_id, Some("123 Main St"), Some("approved")).await;
        let second_mention = insert_mention(&pool, chunk_id, Some("123 Main St"), Some("deferred")).await;

        let candidate_id = detect_and_flag_status_conflict(&pool, second_mention)
            .await
            .unwrap();
        assert!(candidate_id.is_some());

        let candidate_type: String = sqlx::query_scalar!(
            "SELECT candidate_type FROM review_candidates WHERE id = $1",
            candidate_id.unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(candidate_type, "status_conflict");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn no_conflict_flagged_for_matching_statuses(pool: PgPool) {
        let chunk_id = seed_document_chunk(&pool).await;
        insert_mention(&pool, chunk_id, Some("123 Main St"), Some("approved")).await;
        let second_mention = insert_mention(&pool, chunk_id, Some("123 Main St"), Some("approved")).await;

        let candidate_id = detect_and_flag_status_conflict(&pool, second_mention)
            .await
            .unwrap();
        assert!(candidate_id.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn no_conflict_flagged_for_different_addresses(pool: PgPool) {
        let chunk_id = seed_document_chunk(&pool).await;
        insert_mention(&pool, chunk_id, Some("123 Main St"), Some("approved")).await;
        let second_mention = insert_mention(&pool, chunk_id, Some("456 Oak Ave"), Some("deferred")).await;

        let candidate_id = detect_and_flag_status_conflict(&pool, second_mention)
            .await
            .unwrap();
        assert!(candidate_id.is_none());
    }
}
