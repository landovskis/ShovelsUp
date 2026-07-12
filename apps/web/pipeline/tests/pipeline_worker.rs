use shovelsup_pipeline::extractor::llm::AnthropicProvider;
use shovelsup_pipeline::parser::ocr::TesseractOcrProvider;
use shovelsup_pipeline::worker::run_due_fetch_jobs;
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Seeds a municipality whose agenda_url points at the mock server's
/// listing endpoint, with the mock server's host allowlisted.
async fn seed_test_municipality_with_agenda_url(pool: &PgPool, base_url: &str) -> Uuid {
    let host = reqwest::Url::parse(base_url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist, agenda_url) \
         VALUES ('Test City', 'test-city', ARRAY[$1], $2) RETURNING id",
        host,
        format!("{base_url}/listing")
    )
    .fetch_one(pool)
    .await
    .expect("seed municipality")
}

const LISTING_HTML: &str = r#"<html><body>
<a href="/docs/fichier.pdf?typeDoc=pv&doc=1">PV 1</a>
<a href="/docs/fichier.pdf?typeDoc=odj&doc=2">Agenda (ignored)</a>
</body></html>"#;

const REAL_MINUTES_TEXT: &str =
    "CM26 0046 — Approuver le projet d'acte, par lequel la Ville vend à la Coopérative \
     d'habitation Monde-Uni, à des fins d'habitation, notamment de logement social, un \
     immeuble situé au 7965, boulevard de l'Acadie. Adopté à l'unanimité.";

/// End-to-end: a pending job discovers a real-shaped minutes document,
/// fetches it, parses it, and extracts from it.
#[sqlx::test(migrations = "../web/migrations")]
async fn worker_ingests_a_discovered_document_end_to_end(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/listing"))
        .respond_with(ResponseTemplate::new(200).set_body_string(LISTING_HTML))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/docs/fichier.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string(REAL_MINUTES_TEXT),
        )
        .mount(&server)
        .await;

    let municipality_id = seed_test_municipality_with_agenda_url(&pool, &server.uri()).await;
    sqlx::query!(
        "INSERT INTO fetch_jobs (municipality_id, scheduled_for) VALUES ($1, now())",
        municipality_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let ocr = TesseractOcrProvider;
    let llm = AnthropicProvider::from_env();
    let summary = run_due_fetch_jobs(&pool, &ocr, &llm).await.unwrap();

    assert_eq!(summary.documents_ingested, 1);
    assert_eq!(summary.skipped_no_agenda_url, 0);

    let source_doc_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM source_documents WHERE municipality_id = $1",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(source_doc_count, 1, "only the pv link should have been fetched, not odj");

    let job_status: String = sqlx::query_scalar!(
        "SELECT status FROM fetch_jobs WHERE municipality_id = $1",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(job_status, "succeeded");
}

/// Running the worker twice against the same listing page must not
/// re-fetch or re-process an already-ingested document.
#[sqlx::test(migrations = "../web/migrations")]
async fn worker_does_not_refetch_already_ingested_documents(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/listing"))
        .respond_with(ResponseTemplate::new(200).set_body_string(LISTING_HTML))
        .mount(&server)
        .await;
    // .expect(1): if the worker re-fetches on the second run, this mock's
    // implicit call-count verification (via wiremock's `.mount` default)
    // will fail when the server drops at the end of the test.
    Mock::given(method("GET"))
        .and(path("/docs/fichier.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string(REAL_MINUTES_TEXT),
        )
        .expect(1)
        .mount(&server)
        .await;

    let municipality_id = seed_test_municipality_with_agenda_url(&pool, &server.uri()).await;

    sqlx::query!(
        "INSERT INTO fetch_jobs (municipality_id, scheduled_for) VALUES ($1, now())",
        municipality_id
    )
    .execute(&pool)
    .await
    .unwrap();
    let ocr = TesseractOcrProvider;
    let llm = AnthropicProvider::from_env();
    run_due_fetch_jobs(&pool, &ocr, &llm).await.unwrap();

    sqlx::query!(
        "INSERT INTO fetch_jobs (municipality_id, scheduled_for) VALUES ($1, now())",
        municipality_id
    )
    .execute(&pool)
    .await
    .unwrap();
    let second_summary = run_due_fetch_jobs(&pool, &ocr, &llm).await.unwrap();

    assert_eq!(second_summary.documents_ingested, 0);
    assert_eq!(second_summary.documents_skipped_duplicate, 1);

    let source_doc_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM source_documents WHERE municipality_id = $1",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(source_doc_count, 1, "second run must not create a duplicate row");
}
