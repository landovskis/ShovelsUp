use shovelsup_pipeline::resolver::resolve_mention;
use shovelsup_pipeline::resolver::ResolutionOutcome;
use sqlx::PgPool;
use uuid::Uuid;

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

async fn seed_document_chunk_fr(pool: &PgPool) -> Uuid {
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Ville Test', 'ville-test', ARRAY['ville-test.example']) RETURNING id"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, 'https://ville-test.example/doc', 'chk', ''::bytea, 'text/html') RETURNING id",
        municipality_id
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
         VALUES ($1, 0, 'texte du fragment', 'fr') RETURNING id",
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
    project_type: Option<&str>,
    reference_number: Option<&str>,
) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO project_mentions \
         (document_chunk_id, physical_work, civic_address, project_type, reference_number, scale_units) \
         VALUES ($1, true, $2, $3, $4, 1) RETURNING id",
        chunk_id,
        civic_address,
        project_type,
        reference_number,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// TC-REQ-005-3: zero-match mention creates a new project.
#[sqlx::test(migrations = "../web/migrations")]
async fn zero_match_mention_creates_new_project(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let mention_id = insert_mention(
        &pool,
        chunk_id,
        Some("123 Main St"),
        Some("residential"),
        None,
    )
    .await;

    let outcome = resolve_mention(&pool, mention_id).await.unwrap();
    let project_id = match outcome {
        ResolutionOutcome::NewProject { project_id } => project_id,
        other => panic!("expected NewProject, got {other:?}"),
    };

    let linked_project: Option<Uuid> = sqlx::query_scalar!(
        "SELECT project_id FROM project_mentions WHERE id = $1",
        mention_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked_project, Some(project_id));

    let event_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM project_timeline_events WHERE project_id = $1",
        project_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(event_count, 1);
}

/// TC-REQ-005-1: matching address+type links to an existing project.
#[sqlx::test(migrations = "../web/migrations")]
async fn matching_address_and_type_links_to_existing_project(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let first = insert_mention(
        &pool,
        chunk_id,
        Some("123 Main St"),
        Some("residential"),
        None,
    )
    .await;
    let first_outcome = resolve_mention(&pool, first).await.unwrap();
    let existing_project_id = match first_outcome {
        ResolutionOutcome::NewProject { project_id } => project_id,
        other => panic!("expected NewProject, got {other:?}"),
    };

    let second = insert_mention(
        &pool,
        chunk_id,
        Some("123 Main Street"),
        Some("residential"),
        None,
    )
    .await;
    let second_outcome = resolve_mention(&pool, second).await.unwrap();
    assert_eq!(
        second_outcome,
        ResolutionOutcome::Linked {
            project_id: existing_project_id
        }
    );
}

/// TC-REQ-005-2: near-miss address does not auto-link.
#[sqlx::test(migrations = "../web/migrations")]
async fn near_miss_address_does_not_auto_link(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let first = insert_mention(
        &pool,
        chunk_id,
        Some("123 Main St"),
        Some("residential"),
        None,
    )
    .await;
    resolve_mention(&pool, first).await.unwrap();

    let second = insert_mention(
        &pool,
        chunk_id,
        Some("125 Main St"),
        Some("residential"),
        None,
    )
    .await;
    let outcome = resolve_mention(&pool, second).await.unwrap();
    assert!(
        matches!(outcome, ResolutionOutcome::NewProject { .. }),
        "a near-miss address must create its own project, not link to the existing one, got {outcome:?}"
    );
}

/// TC-REQ-005-4: multi-match on address+type creates a review candidate.
///
/// The partial unique index on projects(civic_address_normalized,
/// project_type) makes an exact (address, type) match structurally
/// singular — real ambiguity instead arises when the same address already
/// has a project of a *different* type (e.g. a site redeveloped from
/// industrial to residential): the resolver can't tell whether this new
/// mention continues that project under a corrected type or is a distinct
/// new project at the same address, so it must not guess either way.
#[sqlx::test(migrations = "../web/migrations")]
async fn multi_match_creates_review_candidate(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let first = insert_mention(
        &pool,
        chunk_id,
        Some("500 Industrial Way"),
        Some("industrial"),
        None,
    )
    .await;
    resolve_mention(&pool, first).await.unwrap();

    let second = insert_mention(
        &pool,
        chunk_id,
        Some("500 Industrial Way"),
        Some("residential"),
        None,
    )
    .await;
    let outcome = resolve_mention(&pool, second).await.unwrap();
    let review_candidate_id = match outcome {
        ResolutionOutcome::FlaggedAmbiguous {
            review_candidate_id,
        } => review_candidate_id,
        other => panic!("expected FlaggedAmbiguous, got {other:?}"),
    };

    let candidate_type: String = sqlx::query_scalar!(
        "SELECT candidate_type FROM review_candidates WHERE id = $1",
        review_candidate_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(candidate_type, "ambiguous_match");

    let linked_project: Option<Uuid> = sqlx::query_scalar!(
        "SELECT project_id FROM project_mentions WHERE id = $1",
        second
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        linked_project, None,
        "an ambiguous mention must not be auto-linked"
    );
}

/// An exact repeat (address, type) match must link, not flag as ambiguous —
/// guards against the multi-match fix above over-triggering.
#[sqlx::test(migrations = "../web/migrations")]
async fn exact_repeat_match_links_without_ambiguity(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let first = insert_mention(
        &pool,
        chunk_id,
        Some("500 Industrial Way"),
        Some("industrial"),
        None,
    )
    .await;
    resolve_mention(&pool, first).await.unwrap();
    let second = insert_mention(
        &pool,
        chunk_id,
        Some("500 Industrial Way"),
        Some("industrial"),
        None,
    )
    .await;
    let outcome = resolve_mention(&pool, second).await.unwrap();
    assert!(
        matches!(outcome, ResolutionOutcome::Linked { .. }),
        "an exact repeat match must link, not flag as ambiguous, got {outcome:?}"
    );
}

/// Explicit cross-reference (matching reference_number) takes priority over
/// address+type matching and links even when the address differs.
#[sqlx::test(migrations = "../web/migrations")]
async fn explicit_cross_reference_links_regardless_of_address(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let first = insert_mention(
        &pool,
        chunk_id,
        Some("123 Main St"),
        Some("residential"),
        Some("Application No. 2026-045"),
    )
    .await;
    let first_outcome = resolve_mention(&pool, first).await.unwrap();
    let project_id = match first_outcome {
        ResolutionOutcome::NewProject { project_id } => project_id,
        other => panic!("expected NewProject, got {other:?}"),
    };

    // Second mention has a *different* address but the same reference
    // number — cross-ref must win over the (non-matching) address.
    let second = insert_mention(
        &pool,
        chunk_id,
        Some("999 Different Ave"),
        Some("commercial"),
        Some("Application No. 2026-045"),
    )
    .await;
    let second_outcome = resolve_mention(&pool, second).await.unwrap();
    assert_eq!(second_outcome, ResolutionOutcome::Linked { project_id });
}

#[sqlx::test(migrations = "../web/migrations")]
async fn mention_missing_address_or_type_is_not_resolved(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let mention_id = insert_mention(&pool, chunk_id, None, Some("residential"), None).await;
    let outcome = resolve_mention(&pool, mention_id).await.unwrap();
    assert_eq!(outcome, ResolutionOutcome::InsufficientData);
}

/// Multi-mention project history: 3 mentions of the same project resolve
/// into 1 project with 3 timeline events.
#[sqlx::test(migrations = "../web/migrations")]
async fn multi_mention_project_produces_ordered_timeline(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let m1 = insert_mention(
        &pool,
        chunk_id,
        Some("77 Sport St"),
        Some("institutional"),
        None,
    )
    .await;
    let outcome1 = resolve_mention(&pool, m1).await.unwrap();
    let project_id = match outcome1 {
        ResolutionOutcome::NewProject { project_id } => project_id,
        other => panic!("expected NewProject, got {other:?}"),
    };

    let m2 = insert_mention(
        &pool,
        chunk_id,
        Some("77 Sport St"),
        Some("institutional"),
        None,
    )
    .await;
    resolve_mention(&pool, m2).await.unwrap();
    let m3 = insert_mention(
        &pool,
        chunk_id,
        Some("77 Sport St"),
        Some("institutional"),
        None,
    )
    .await;
    resolve_mention(&pool, m3).await.unwrap();

    let project_count: i64 =
        sqlx::query_scalar!("SELECT count(*) FROM projects WHERE id = $1", project_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(project_count, 1);

    let event_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM project_timeline_events WHERE project_id = $1",
        project_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(event_count, 3);
}

/// IMP-REQ-005-07 concurrency: simultaneous resolution of two mentions at a
/// brand-new (address, type) must not produce two projects or duplicate
/// timeline rows — the unique-index race is resolved to a single winner.
#[sqlx::test(migrations = "../web/migrations")]
async fn concurrent_resolution_of_new_address_produces_one_project(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let m1 = insert_mention(
        &pool,
        chunk_id,
        Some("42 Concurrent Blvd"),
        Some("mixed-use"),
        None,
    )
    .await;
    let m2 = insert_mention(
        &pool,
        chunk_id,
        Some("42 Concurrent Blvd"),
        Some("mixed-use"),
        None,
    )
    .await;

    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { resolve_mention(&pool1, m1).await }),
        tokio::spawn(async move { resolve_mention(&pool2, m2).await }),
    );
    r1.unwrap().unwrap();
    r2.unwrap().unwrap();

    let project_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM projects WHERE civic_address_normalized = '42 concurrent boulevard' AND project_type = 'mixed-use'"
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        project_count, 1,
        "concurrent resolution of the same new address must produce exactly one project"
    );

    let both_linked: i64 = sqlx::query_scalar!(
        "SELECT count(DISTINCT project_id) FROM project_mentions WHERE id = ANY($1)",
        &[m1, m2]
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        both_linked, 1,
        "both mentions must link to the same project"
    );
}

/// IMP-REQ-007-02 wiring: `resolve_mention` dispatches to the French-Quebec
/// address normalizer (not the English one) for a mention whose source
/// chunk is language='fr', so two differently-formatted French addresses
/// for the same civic location link to one project. Exercises
/// `resolve_mention`/`try_resolve` directly, not just the standalone
/// `normalize_address_fr` function.
#[sqlx::test(migrations = "../web/migrations")]
async fn french_mention_addresses_resolve_via_the_french_normalizer(pool: PgPool) {
    let chunk_id = seed_document_chunk_fr(&pool).await;
    let m1 = insert_mention(
        &pool,
        chunk_id,
        Some("456, boul. Saint-Laurent"),
        Some("residential"),
        None,
    )
    .await;
    let m2 = insert_mention(
        &pool,
        chunk_id,
        Some("456 Boulevard Saint-Laurent"),
        Some("residential"),
        None,
    )
    .await;

    let outcome1 = resolve_mention(&pool, m1).await.unwrap();
    let project_id = match outcome1 {
        ResolutionOutcome::NewProject { project_id } => project_id,
        other => panic!("expected NewProject, got {other:?}"),
    };

    let outcome2 = resolve_mention(&pool, m2).await.unwrap();
    match outcome2 {
        ResolutionOutcome::Linked {
            project_id: linked_id,
        } => {
            assert_eq!(linked_id, project_id, "the comma/abbreviation variant must normalize to the same project as the first mention");
        }
        other => panic!("expected Linked to the existing project, got {other:?}"),
    }

    let normalized: String = sqlx::query_scalar!(
        "SELECT civic_address_normalized FROM projects WHERE id = $1",
        project_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .expect("normalized address must be set");
    assert_eq!(
        normalized, "456 boulevard saint-laurent",
        "must use the French normalizer's canonical form, not the English one"
    );
}
