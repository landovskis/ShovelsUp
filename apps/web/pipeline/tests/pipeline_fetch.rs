use shovelsup_pipeline::fetcher::{FetchError, FetchOutcome, Fetcher};
use sqlx::PgPool;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn seed_test_municipality(pool: &PgPool, allowed_host: &str) -> uuid::Uuid {
    sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Test City', 'test-city', ARRAY[$1]) RETURNING id",
        allowed_host
    )
    .fetch_one(pool)
    .await
    .expect("seed municipality")
}

/// TC-REQ-001-1: Fetch succeeds for a valid allowlisted URL.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_tc_req_001_1_fetch_succeeds_for_allowlisted_url(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string("agenda body"))
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;

    let fetcher = Fetcher::new();
    let url = format!("{}/agenda.html", server.uri());
    let outcome = fetcher
        .fetch(&pool, municipality_id, &url)
        .await
        .expect("fetch should succeed");

    match outcome {
        FetchOutcome::Fetched { document_id } => {
            let count: i64 = sqlx::query_scalar!(
                "SELECT count(*) FROM source_documents WHERE id = $1",
                document_id
            )
            .fetch_one(&pool)
            .await
            .unwrap()
            .unwrap();
            assert_eq!(count, 1);
        }
        FetchOutcome::Duplicate { .. } => panic!("expected a fresh fetch, got duplicate"),
    }
}

/// TC-REQ-001-2: Fetch is a no-op on identical checksum (dedupe).
#[sqlx::test(migrations = "../web/migrations")]
async fn test_tc_req_001_2_fetch_dedupes_identical_checksum(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string("identical body"))
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;
    let fetcher = Fetcher::new();
    let url = format!("{}/agenda.html", server.uri());

    let first = fetcher.fetch(&pool, municipality_id, &url).await.unwrap();
    let second = fetcher.fetch(&pool, municipality_id, &url).await.unwrap();

    let first_id = match first {
        FetchOutcome::Fetched { document_id } => document_id,
        FetchOutcome::Duplicate { .. } => panic!("first fetch should not be a duplicate"),
    };
    match second {
        FetchOutcome::Duplicate { document_id } => assert_eq!(document_id, first_id),
        FetchOutcome::Fetched { .. } => panic!("second fetch should dedupe, not insert"),
    }

    let count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM source_documents WHERE municipality_id = $1",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(count, 1, "dedupe must not create a second row");
}

/// TC-REQ-001-3: Fetch rejects a non-allowlisted domain before any HTTP request.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_tc_req_001_3_fetch_rejects_non_allowlisted_domain(pool: PgPool) {
    let municipality_id = seed_test_municipality(&pool, "only-this-host.example").await;
    let fetcher = Fetcher::new();

    let result = fetcher
        .fetch(&pool, municipality_id, "https://not-allowlisted.example/x")
        .await;

    match result {
        Err(FetchError::NotAllowlisted(host)) => assert_eq!(host, "not-allowlisted.example"),
        other => panic!("expected NotAllowlisted, got {other:?}"),
    }
}

/// TC-REQ-001-4: Fetch recovers from source 503 via retry/backoff.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_tc_req_001_4_fetch_recovers_from_503_via_retry(pool: PgPool) {
    let server = MockServer::start().await;
    // First two requests 503, third succeeds.
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string("eventually ok"))
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;
    let fetcher = Fetcher::new();
    let url = format!("{}/agenda.html", server.uri());

    let outcome = fetcher
        .fetch(&pool, municipality_id, &url)
        .await
        .expect("fetch should eventually succeed after retries");
    assert!(matches!(outcome, FetchOutcome::Fetched { .. }));
}

/// A 4xx response is a permanent client error, not a transient failure —
/// it must not be retried.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_fetch_does_not_retry_4xx_responses(pool: PgPool) {
    let server = MockServer::start().await;
    // If the fetcher retried, this mock's default expectation of at most 1
    // call (wiremock's implicit `.expect(1)` via `.mount` without
    // `up_to_n_times`) would fail verification when the mock server drops.
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;
    let fetcher = Fetcher::new();
    let url = format!("{}/agenda.html", server.uri());

    let result = fetcher.fetch(&pool, municipality_id, &url).await;
    assert!(
        matches!(result, Err(FetchError::Http(_))),
        "expected an immediate Http error for a 404, got {result:?}"
    );
}

/// Sustained 5xx failures exhaust MAX_ATTEMPTS and surface as an error rather
/// than retrying forever.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_fetch_gives_up_after_max_attempts(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;
    let fetcher = Fetcher::new();
    let url = format!("{}/agenda.html", server.uri());

    let result = fetcher.fetch(&pool, municipality_id, &url).await;
    assert!(
        matches!(result, Err(FetchError::Http(_))),
        "expected retries to be exhausted with an Http error, got {result:?}"
    );
}

/// Fetching against a municipality that doesn't exist is a distinct error
/// from "not allowlisted".
#[sqlx::test(migrations = "../web/migrations")]
async fn test_fetch_reports_missing_municipality(pool: PgPool) {
    let fetcher = Fetcher::new();
    let result = fetcher
        .fetch(&pool, uuid::Uuid::new_v4(), "https://example.com/x")
        .await;
    assert!(matches!(result, Err(FetchError::MunicipalityNotFound(_))));
}

/// A redirect response must not be followed transparently — see the SSRF
/// note on `Fetcher::new`. It should surface as an HTTP error rather than
/// silently fetching whatever the `Location` header points at.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_fetch_does_not_follow_redirects(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/agenda.html"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://internal.invalid/secret"),
        )
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;
    let fetcher = Fetcher::new();
    let url = format!("{}/agenda.html", server.uri());

    let result = fetcher.fetch(&pool, municipality_id, &url).await;
    assert!(
        matches!(result, Err(FetchError::UnexpectedRedirect { .. })),
        "redirect must not be followed, got {result:?}"
    );
}

/// fetch_bytes must return the raw body without persisting a source_documents row.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_fetch_bytes_returns_body_without_persisting(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/index.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string("index body"))
        .mount(&server)
        .await;

    let host = reqwest::Url::parse(&server.uri())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let municipality_id = seed_test_municipality(&pool, &host).await;
    let fetcher = Fetcher::new();
    let url = format!("{}/index.html", server.uri());

    let bytes = fetcher
        .fetch_bytes(&pool, municipality_id, &url)
        .await
        .expect("fetch_bytes should succeed");
    assert_eq!(bytes, b"index body");

    let count: i64 = sqlx::query_scalar!("SELECT count(*) FROM source_documents")
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 0, "fetch_bytes must not persist a source_documents row");
}

/// fetch_bytes still enforces the domain allowlist.
#[sqlx::test(migrations = "../web/migrations")]
async fn test_fetch_bytes_rejects_non_allowlisted_domain(pool: PgPool) {
    let municipality_id = seed_test_municipality(&pool, "only-this-host.example").await;
    let fetcher = Fetcher::new();

    let result = fetcher
        .fetch_bytes(&pool, municipality_id, "https://not-allowlisted.example/x")
        .await;

    assert!(matches!(result, Err(FetchError::NotAllowlisted(_))));
}
