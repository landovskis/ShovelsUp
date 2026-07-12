# Fetch-Job Worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the REQ-001 data pipeline actually run in production for Montreal: discover real procès-verbal (minutes) documents from the city's real document-listing page, fetch/parse/extract each one, and run the whole thing on a recurring in-process schedule.

**Architecture:** `apps/web` is now a Cargo workspace of three crates (`domain`, `pipeline` = `shovelsup-pipeline`, `web` = `shovelsup-web`); this plan's code lives almost entirely in the `pipeline` crate. A pure link-extraction function (`worker::core::extract_pv_document_links`) finds real minutes-document URLs on Montreal's listing page; a shell (`worker::run_due_fetch_jobs`) drives each pending `fetch_jobs` row through discovery → fetch (deduped against already-ingested `source_documents.source_url`) → parse → extract; a `tokio::spawn` interval loop in the `web` crate's `main.rs` calls the existing `Scheduler` and the new worker every hour, gated by a live-read `DATA_PIPELINE_INGESTION_ENABLED` flag (already defined in `web`).

**Tech Stack:** Rust/Axum/sqlx (Postgres), `scraper` (already a `pipeline` dependency) for HTML link extraction, `wiremock` for integration tests, `tokio::time::interval` for scheduling.

**Note on this revision:** this plan was originally written against a single-crate `apps/web/src/...` layout. Before Task 1 was implemented, `main` was restructured into the three-crate workspace described above, with a mandated "functional core / imperative shell" pattern per module (see `apps/web/CLAUDE.md`): deterministic, data-in/data-out logic goes in a `<module>/core.rs` submodule with no I/O and no DB access in its tests; SQLx queries, HTTP calls, env reads, clocks, and retries stay in the parent `<module>.rs` shell. `pipeline/src/scheduler.rs` + `pipeline/src/scheduler/core.rs` is the reference example — this plan's `worker` module follows the same split. All paths below are the current, correct ones.

## Global Constraints

- All apps must support English and French (project CLAUDE.md) — not applicable to this backend-only worker (no user-facing strings added).
- Every fallible step inside the spawned interval loop must be matched and logged (`tracing::error!`), never `.unwrap()`'d or propagated with `?` — a single bad tick must never kill the process.
- New `sqlx::query!`/`query_scalar!` macros require a migrated dev database reachable via `DATABASE_URL` at compile time, and `cargo sqlx prepare --workspace` must be re-run and its output (under `apps/web/.sqlx/`, shared workspace-wide) committed after adding or changing any query.
- Only `typeDoc=pv` links are ever fetched — `odj` (agenda) and `da` (attachment) links are discovered but discarded.
- No new table for "already discovered" tracking — reuse `source_documents.source_url`.
- Toronto/Vancouver `agenda_url` stays `NULL` — out of scope (design doc Non-goals).
- Follow the functional-core/imperative-shell convention: any new deterministic, testable-without-I/O logic goes in a `core` submodule; pipeline submodules default to `pub(crate)`, widening to `pub` only for what another crate (`web`) actually calls.

---

## File Structure

| File | Responsibility |
|---|---|
| `apps/web/web/migrations/015_montreal_agenda_url.sql` | Adds `municipalities.agenda_url`, seeds Montreal's real listing-page URL |
| `apps/web/web/src/config/flags.rs` (modify) | Adds `data_pipeline_ingestion_enabled()`, reusing the existing `is_truthy` helper |
| `apps/web/pipeline/src/fetcher.rs` (modify) | Extracts shared allowlist-check logic; adds `Fetcher::fetch_bytes` (fetch without persisting) |
| `apps/web/pipeline/src/worker.rs` (new) | Shell: `run_due_fetch_jobs` — drives pending `fetch_jobs` rows through discovery/fetch/parse/extract |
| `apps/web/pipeline/src/worker/core.rs` (new) | Core: pure `extract_pv_document_links` — no I/O, unit-tested against a real fixture |
| `apps/web/pipeline/src/lib.rs` (modify) | Declares `pub mod worker;` |
| `apps/web/web/src/main.rs` (modify) | Spawns the hourly interval loop calling `Scheduler` + `worker` |
| `apps/web/pipeline/tests/fixtures/montreal_listing_page.html` | Real captured HTML from Montreal's document-listing page (already saved, needs copying into new path) |
| `apps/web/pipeline/tests/pipeline_worker.rs` (new) | Integration tests for the worker, end to end |
| `docs/runbooks/data_pipeline_ingestion.md` (modify) | Update "Current implementation status" — worker now exists |
| `apps/web/IMPLEMENTATION_CHECKLIST.md` (modify) | Add `IMP-REQ-001-11`/`-12`/`-13` task entries |

---

### Task 1: Migration — `agenda_url` column + Montreal seed

**Files:**
- Create: `apps/web/web/migrations/015_montreal_agenda_url.sql`
- Test: `apps/web/pipeline/tests/pipeline_scheduler.rs` (existing tests must still pass unmodified — confirms the migration is additive-only)

**Interfaces:**
- Produces: `municipalities.agenda_url` (nullable `TEXT`), populated for Montreal only.

- [ ] **Step 1: Write the migration**

```sql
-- IMP-REQ-001-11: worker needs a real URL to start from. Montreal's actual
-- document index (found by following the link from the marketing page at
-- montreal.ca/conseils-decisionnels/conseil-municipal) is this legacy
-- portal page — verified reachable and containing real typeDoc=pv links via
-- direct curl, 2026-07-11. Toronto/Vancouver stay NULL (documented gap, see
-- docs/superpowers/specs/2026-07-11-fetch-job-worker-design.md Non-goals).
ALTER TABLE municipalities ADD COLUMN agenda_url TEXT;

UPDATE municipalities
SET agenda_url = 'https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL'
WHERE slug = 'montreal';
```

- [ ] **Step 2: Bring up the dev database and apply migrations**

Run: `cd apps/web && docker compose up -d postgres && sleep 2 && cd web && DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup sqlx migrate run`
Expected: `Applied 15/migrate montreal agenda url` (or similar) with no errors. (Migrations live under `apps/web/web/migrations`, and `sqlx migrate run` defaults to `./migrations` relative to cwd, hence `cd web` first.)

- [ ] **Step 3: Verify the seed**

Run: `psql "postgres://shovelsup:change-me@localhost:5434/shovelsup" -c "SELECT slug, agenda_url FROM municipalities ORDER BY slug;"`
Expected: `montreal` row has the `ville.montreal.qc.ca` URL; `toronto`/`vancouver` rows have `NULL`.

- [ ] **Step 4: Run the existing scheduler tests to confirm nothing broke**

Run (from `apps/web/`): `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo test -p shovelsup-pipeline --test pipeline_scheduler`
Expected: all 4 existing tests pass unchanged (the migration is additive-only, doesn't touch `fetch_jobs`).

- [ ] **Step 5: Commit**

```bash
git add apps/web/web/migrations/015_montreal_agenda_url.sql
git commit -m "feat(imp-req-001-11): add municipalities.agenda_url, seed Montreal's real listing page"
```

---

### Task 2: `DATA_PIPELINE_INGESTION_ENABLED` flag

**Files:**
- Modify: `apps/web/web/src/config/flags.rs`

**Interfaces:**
- Produces: `pub fn data_pipeline_ingestion_enabled() -> bool` in `crate::config::flags` (crate = `shovelsup-web`).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `apps/web/web/src/config/flags.rs`:

```rust
    #[test]
    fn data_pipeline_ingestion_disabled_when_unset_or_falsy() {
        assert!(!is_truthy(None));
        // data_pipeline_ingestion_enabled() itself reads the real env var,
        // so it's exercised indirectly via is_truthy here (same helper,
        // same contract as review_queue_enabled) — a dedicated env-var
        // integration test would be flaky under parallel test execution
        // (shared process env), matching why review_queue_enabled has no
        // such test either.
    }
```

- [ ] **Step 2: Run test to verify current state**

Run (from `apps/web/`): `cargo test -p shovelsup-web --lib config::flags`
Expected: existing `is_truthy` tests still pass (this step is a no-op check since Step 1 didn't add new assertions — proceed to implementation).

- [ ] **Step 3: Add the flag function**

In `apps/web/web/src/config/flags.rs`, add after `review_queue_enabled`:

```rust
/// `DATA_PIPELINE_INGESTION_ENABLED` (IMP-REQ-001-12, default `false`/unset):
/// gates the fetch-job worker's interval loop in `main.rs`. Read live on
/// every tick (not cached at startup) so ops can flip it without a restart
/// (docs/runbooks/data_pipeline_ingestion.md). Do not enable in an
/// environment where the seeded municipality domains
/// (migrations/002_seed_municipalities.sql) haven't had legal/public-source
/// sign-off.
pub fn data_pipeline_ingestion_enabled() -> bool {
    is_truthy(std::env::var("DATA_PIPELINE_INGESTION_ENABLED").ok().as_deref())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p shovelsup-web --lib config::flags`
Expected: `test config::flags::tests::... ok` for all flags tests.

- [ ] **Step 5: Commit**

```bash
git add apps/web/web/src/config/flags.rs
git commit -m "feat(imp-req-001-12): wire DATA_PIPELINE_INGESTION_ENABLED as a live-read flag"
```

---

### Task 3: Refactor `Fetcher` to share allowlist logic; add `fetch_bytes`

**Files:**
- Modify: `apps/web/pipeline/src/fetcher.rs`
- Modify: `apps/web/pipeline/tests/pipeline_fetch.rs`

**Interfaces:**
- Consumes: nothing new (internal refactor of existing `Fetcher`).
- Produces: `pub async fn Fetcher::fetch_bytes(&self, pool: &PgPool, municipality_id: Uuid, url: &str) -> Result<Vec<u8>, FetchError>`.

- [ ] **Step 1: Write the failing test**

Add to `apps/web/pipeline/tests/pipeline_fetch.rs` (reuses the file's existing `seed_test_municipality` helper and its existing `Fetcher`/`FetchError` imports — no new `use` needed):

```rust
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
```

Note the `migrations = "../web/migrations"` path: `pipeline`'s integration tests run with cwd = `apps/web/pipeline/`, and the actual migration files live in the sibling `web` crate. Check the file's existing `#[sqlx::test(migrations = ...)]` attribute on an existing test in this file first — use whatever relative path it already uses (it must already solve this, since these tests currently pass); only add the two new tests below it using the identical `migrations = "..."` value found there, copy-pasted, not `"../web/migrations"` if the existing tests use something else.

- [ ] **Step 2: Run tests to verify they fail**

Run (from `apps/web/`): `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo test -p shovelsup-pipeline --test pipeline_fetch fetch_bytes`
Expected: FAIL with `no method named 'fetch_bytes' found for struct 'Fetcher'`.

- [ ] **Step 3: Refactor `Fetcher::fetch` and add `fetch_bytes`**

In `apps/web/pipeline/src/fetcher.rs`, replace the existing `pub async fn fetch` method body with:

```rust
    /// Fetch `url` for `municipality_id`, enforcing the domain allowlist and
    /// deduping by checksum against previously stored `source_documents`.
    pub async fn fetch(
        &self,
        pool: &PgPool,
        municipality_id: Uuid,
        url: &str,
    ) -> Result<FetchOutcome, FetchError> {
        self.check_allowlist(pool, municipality_id, url).await?;

        let (body, content_type) = self.fetch_with_retry(url).await?;
        let checksum = format!("{:x}", Sha256::digest(&body));

        if let Some(existing_id) = sqlx::query_scalar!(
            "SELECT id FROM source_documents WHERE municipality_id = $1 AND checksum = $2",
            municipality_id,
            checksum
        )
        .fetch_optional(pool)
        .await?
        {
            return Ok(FetchOutcome::Duplicate {
                document_id: existing_id,
            });
        }

        let document_id = sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
            municipality_id,
            url,
            checksum,
            body,
            content_type,
        )
        .fetch_one(pool)
        .await?;

        Ok(FetchOutcome::Fetched { document_id })
    }

    /// Fetches `url`'s raw bytes for `municipality_id`, enforcing the same
    /// domain allowlist as `fetch`, but without persisting a
    /// `source_documents` row. For index/listing pages consumed only to
    /// discover further document links (`worker::core::extract_pv_document_links`),
    /// not decision-bearing documents in their own right.
    pub async fn fetch_bytes(
        &self,
        pool: &PgPool,
        municipality_id: Uuid,
        url: &str,
    ) -> Result<Vec<u8>, FetchError> {
        self.check_allowlist(pool, municipality_id, url).await?;
        let (body, _content_type) = self.fetch_with_retry(url).await?;
        Ok(body)
    }

    async fn check_allowlist(
        &self,
        pool: &PgPool,
        municipality_id: Uuid,
        url: &str,
    ) -> Result<(), FetchError> {
        let parsed = reqwest::Url::parse(url).map_err(|_| FetchError::InvalidUrl(url.to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| FetchError::InvalidUrl(url.to_string()))?
            .to_string();

        let allowlist: Vec<String> = sqlx::query_scalar!(
            "SELECT domain_allowlist FROM municipalities WHERE id = $1",
            municipality_id
        )
        .fetch_optional(pool)
        .await?
        .ok_or(FetchError::MunicipalityNotFound(municipality_id))?;

        if !is_allowlisted(&host, &allowlist) {
            return Err(FetchError::NotAllowlisted(host));
        }
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo test -p shovelsup-pipeline --test pipeline_fetch`
Expected: all existing `pipeline_fetch` tests (TC-REQ-001-1 through -4 and the others) plus the 2 new `fetch_bytes` tests pass — the refactor must not change `fetch`'s observable behavior.

- [ ] **Step 5: Commit**

```bash
git add apps/web/pipeline/src/fetcher.rs apps/web/pipeline/tests/pipeline_fetch.rs
git commit -m "refactor(imp-req-001-11): share Fetcher's allowlist check, add fetch_bytes"
```

---

### Task 4: `worker::core::extract_pv_document_links` (pure core module)

**Files:**
- Create: `apps/web/pipeline/src/worker.rs` (shell — this task only adds the module declaration and re-export scaffolding; the shell logic itself is Task 5)
- Create: `apps/web/pipeline/src/worker/core.rs`
- Modify: `apps/web/pipeline/src/lib.rs` (add `pub mod worker;`)
- Fixture: `apps/web/pipeline/tests/fixtures/montreal_listing_page.html` (copy from the design-phase capture — see Step 1)

**Interfaces:**
- Produces: `pub(crate) fn extract_pv_document_links(html: &str, base_url: &str) -> Vec<String>` in `crate::worker::core` (visible within the `pipeline` crate only — only `worker.rs`'s shell calls it, nothing in `web` needs it directly).

- [ ] **Step 1: Ensure the fixture file is present at its new path**

Run: `mkdir -p apps/web/pipeline/tests/fixtures && ls apps/web/pipeline/tests/fixtures/montreal_listing_page.html 2>&1`

If missing, fetch it fresh (it's real, verified content — do not fabricate a substitute): `curl -sL -A "Mozilla/5.0" "https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL" -o apps/web/pipeline/tests/fixtures/montreal_listing_page.html`

Then verify: `wc -c apps/web/pipeline/tests/fixtures/montreal_listing_page.html` — expect roughly 59740 bytes (the page is live content and may have changed slightly since 2026-07-11; if the byte count is wildly different, re-run Step 2's test and adjust the expected doc-id list to whatever `typeDoc=pv` IDs are actually present, rather than assume the numbers below are still exact).

- [ ] **Step 2: Write the module with its test module**

Create `apps/web/pipeline/src/worker/core.rs`:

```rust
use scraper::{Html, Selector};

/// Extracts absolute URLs for `typeDoc=pv` (procès-verbal/minutes) links
/// from `html`, resolving relative hrefs against `base_url`. Ignores
/// `typeDoc=odj` (agenda, pre-decision) and `typeDoc=da` (attachment) links
/// — only minutes carry the recorded decision text
/// (`approval_status_raw`) this product surfaces (see the design doc's
/// Non-goals). Returns an empty `Vec` if `base_url` itself doesn't parse or
/// no matching links are found — never panics on malformed input.
pub(crate) fn extract_pv_document_links(html: &str, base_url: &str) -> Vec<String> {
    let Ok(base) = reqwest::Url::parse(base_url) else {
        return Vec::new();
    };

    let document = Html::parse_document(html);
    // Safe to unwrap: this selector is a fixed, valid CSS string.
    let selector = Selector::parse("a[href]").unwrap();

    document
        .select(&selector)
        .filter_map(|el| el.value().attr("href"))
        .filter(|href| href.to_lowercase().contains("typedoc=pv"))
        .filter_map(|href| base.join(href).ok())
        .map(|url| url.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_URL: &str =
        "https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL";

    fn real_fixture_html() -> String {
        let bytes = std::fs::read("tests/fixtures/montreal_listing_page.html")
            .expect("fixture file must exist — see Task 4 Step 1");
        // The real page is windows-1252-encoded; lossy UTF-8 decoding
        // mangles accented characters but leaves the pure-ASCII href
        // attributes (what this function reads) untouched.
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn extracts_exactly_the_known_pv_links_from_the_real_fixture() {
        let html = real_fixture_html();
        let links = extract_pv_document_links(&html, BASE_URL);

        let expected_doc_ids = ["8262", "8294", "8295", "8329", "8354", "8378", "8423"];
        assert_eq!(links.len(), expected_doc_ids.len());
        for doc_id in expected_doc_ids {
            let expected_url = format!(
                "https://ville.montreal.qc.ca/sel/adi-public/afficherpdf/fichier.pdf?typeDoc=pv&doc={doc_id}"
            );
            assert!(
                links.contains(&expected_url),
                "expected {expected_url} in {links:?}"
            );
        }
    }

    #[test]
    fn ignores_odj_and_da_links() {
        let html = real_fixture_html();
        let links = extract_pv_document_links(&html, BASE_URL);
        assert!(links.iter().all(|l| l.contains("typeDoc=pv")));
        assert!(!links.iter().any(|l| l.contains("typeDoc=odj")));
        assert!(!links.iter().any(|l| l.contains("typeDoc=da")));
    }

    #[test]
    fn returns_empty_for_html_with_no_links() {
        let links = extract_pv_document_links("<html><body>no links here</body></html>", BASE_URL);
        assert!(links.is_empty());
    }

    #[test]
    fn returns_empty_for_unparseable_base_url() {
        let html = r#"<a href="/sel/adi-public/afficherpdf/fichier.pdf?typeDoc=pv&doc=1">PV</a>"#;
        let links = extract_pv_document_links(html, "not a url");
        assert!(links.is_empty());
    }
}
```

Create `apps/web/pipeline/src/worker.rs` (shell scaffold only — Task 5 fills in `run_due_fetch_jobs`; matches the `scheduler.rs`/`scheduler/core.rs` split exactly):

```rust
pub(crate) mod core;
```

- [ ] **Step 3: Register the module**

In `apps/web/pipeline/src/lib.rs`, add (alphabetically among the existing `pub mod` lines):

```rust
pub mod worker;
```

- [ ] **Step 4: Run tests**

Run: `cd apps/web && cargo test -p shovelsup-pipeline --lib worker::core`
Expected: all 4 tests pass. If `extracts_exactly_the_known_pv_links_from_the_real_fixture` fails on doc-id mismatch, re-check Step 1's fixture — the listing page is live content and may have changed since this plan was written; adjust the expected doc-id list to match reality rather than force the old numbers.

- [ ] **Step 5: Commit**

```bash
git add apps/web/pipeline/src/worker.rs apps/web/pipeline/src/worker/core.rs apps/web/pipeline/src/lib.rs apps/web/pipeline/tests/fixtures/montreal_listing_page.html
git commit -m "feat(imp-req-001-11): extract real procès-verbal links from Montreal's document listing"
```

---

### Task 5: `worker::run_due_fetch_jobs` — shell logic + unit test

**Files:**
- Modify: `apps/web/pipeline/src/worker.rs` (add the shell logic; `pub(crate) mod core;` from Task 4 stays at the top)

**Interfaces:**
- Consumes: `Fetcher::fetch`, `Fetcher::fetch_bytes` (Task 3), `worker::core::extract_pv_document_links` (Task 4), `parser::orchestrate::parse_and_store`, `extractor::extract_and_store`, `parser::ocr::OcrProvider`, `extractor::llm::LlmProvider` — all accessed as `crate::...` (same crate now, not `super::...`).
- Produces: `pub struct WorkerSummary { pub documents_ingested: usize, pub documents_skipped_duplicate: usize, pub failed: usize, pub skipped_no_agenda_url: usize }` and `pub async fn run_due_fetch_jobs(pool: &PgPool, ocr: &dyn OcrProvider, llm: &dyn LlmProvider) -> Result<WorkerSummary, sqlx::Error>` in `crate::worker` (must be `pub` — called from the `web` crate in Task 8).

- [ ] **Step 1: Write the failing unit test**

Add to `apps/web/pipeline/src/worker.rs` (below the existing `pub(crate) mod core;` line from Task 4):

```rust
use sqlx::PgPool;
use uuid::Uuid;

use crate::extractor::{extract_and_store, llm::LlmProvider};
use crate::fetcher::{FetchOutcome, Fetcher};
use crate::parser::{ocr::OcrProvider, orchestrate::parse_and_store};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct WorkerSummary {
    pub documents_ingested: usize,
    pub documents_skipped_duplicate: usize,
    pub failed: usize,
    pub skipped_no_agenda_url: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A municipality with no agenda_url configured (Toronto/Vancouver
    /// today) must be skipped, not treated as a failure — see the design
    /// doc's worker step 1.
    #[sqlx::test(migrations = "../web/migrations")]
    async fn run_due_fetch_jobs_skips_municipality_with_no_agenda_url(pool: PgPool) {
        // The 002 seed migration inserts Toronto/Vancouver with agenda_url
        // NULL and a pending fetch_jobs row is created for each via the
        // scheduler in a real run; here we insert one directly to isolate
        // this unit from Scheduler's behavior.
        let municipality_id: Uuid = sqlx::query_scalar!(
            "SELECT id FROM municipalities WHERE slug = 'toronto'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO fetch_jobs (municipality_id, scheduled_for) VALUES ($1, now())",
            municipality_id
        )
        .execute(&pool)
        .await
        .unwrap();

        let ocr = crate::parser::ocr::TesseractOcrProvider;
        let llm = crate::extractor::llm::AnthropicProvider::new("unused".to_string());
        let summary = run_due_fetch_jobs(&pool, &ocr, &llm).await.unwrap();

        assert_eq!(summary.skipped_no_agenda_url, 1);

        let status: String = sqlx::query_scalar!(
            "SELECT status FROM fetch_jobs WHERE municipality_id = $1",
            municipality_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "pending", "job with no agenda_url stays pending, not failed");
    }
}
```

Use whichever `migrations = "..."` relative path Task 3's tests ended up using in `pipeline/tests/pipeline_fetch.rs` (they must already have solved this path problem) — copy that exact string here rather than trust `"../web/migrations"` blindly if Task 3 found a different working value.

- [ ] **Step 2: Run test to verify it fails**

Run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo test -p shovelsup-pipeline --lib worker`
Expected: FAIL with `cannot find function 'run_due_fetch_jobs' in this scope`.

- [ ] **Step 3: Implement `run_due_fetch_jobs`**

Add above the `#[cfg(test)]` block in `apps/web/pipeline/src/worker.rs`:

```rust
/// Drives every due, pending `fetch_jobs` row through discovery → fetch →
/// parse → extract. See the design doc
/// (docs/superpowers/specs/2026-07-11-fetch-job-worker-design.md) for the
/// full per-job/per-document state machine.
pub async fn run_due_fetch_jobs(
    pool: &PgPool,
    ocr: &dyn OcrProvider,
    llm: &dyn LlmProvider,
) -> Result<WorkerSummary, sqlx::Error> {
    let mut summary = WorkerSummary::default();
    let fetcher = Fetcher::new();

    let due_jobs: Vec<(Uuid, Uuid)> = sqlx::query!(
        "SELECT id, municipality_id FROM fetch_jobs \
         WHERE status = 'pending' AND scheduled_for <= now()"
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| (row.id, row.municipality_id))
    .collect();

    for (job_id, municipality_id) in due_jobs {
        let agenda_url: Option<String> = sqlx::query_scalar!(
            "SELECT agenda_url FROM municipalities WHERE id = $1",
            municipality_id
        )
        .fetch_one(pool)
        .await?;

        let Some(agenda_url) = agenda_url else {
            summary.skipped_no_agenda_url += 1;
            continue;
        };

        sqlx::query!(
            "UPDATE fetch_jobs SET status = 'in_progress', updated_at = now() WHERE id = $1",
            job_id
        )
        .execute(pool)
        .await?;

        let html_bytes = match fetcher.fetch_bytes(pool, municipality_id, &agenda_url).await {
            Ok(bytes) => bytes,
            Err(err) => {
                sqlx::query!(
                    "UPDATE fetch_jobs SET status = 'failed', attempts = attempts + 1, \
                     last_error = $1, updated_at = now() WHERE id = $2",
                    err.to_string(),
                    job_id
                )
                .execute(pool)
                .await?;
                summary.failed += 1;
                continue;
            }
        };

        let html = String::from_utf8_lossy(&html_bytes).into_owned();
        let document_urls = core::extract_pv_document_links(&html, &agenda_url);

        for document_url in document_urls {
            // fetch_optional + is_some(), not `SELECT EXISTS(...)`: sqlx's
            // nullability inference for a computed EXISTS() expression is
            // not guaranteed to produce Option<bool> vs. plain bool, but a
            // plain column selection's Option-ness is reliably inferred —
            // same pattern as Scheduler::enqueue_due_fetches's
            // `already_scheduled_today` check.
            let already_ingested = sqlx::query_scalar!(
                "SELECT id FROM source_documents WHERE municipality_id = $1 AND source_url = $2",
                municipality_id,
                document_url
            )
            .fetch_optional(pool)
            .await?
            .is_some();

            if already_ingested {
                summary.documents_skipped_duplicate += 1;
                continue;
            }

            match fetcher.fetch(pool, municipality_id, &document_url).await {
                Err(err) => {
                    tracing::warn!(
                        job_id = %job_id,
                        url = %document_url,
                        error = %err,
                        "failed to fetch a discovered document"
                    );
                    summary.failed += 1;
                }
                Ok(FetchOutcome::Duplicate { .. }) => {
                    summary.documents_skipped_duplicate += 1;
                }
                Ok(FetchOutcome::Fetched { document_id }) => {
                    parse_and_store(pool, document_id, ocr).await?;

                    let chunks = sqlx::query!(
                        "SELECT id, content FROM document_chunks WHERE source_document_id = $1",
                        document_id
                    )
                    .fetch_all(pool)
                    .await?;

                    for chunk in chunks {
                        extract_and_store(pool, chunk.id, &chunk.content, llm).await?;
                    }

                    summary.documents_ingested += 1;
                }
            }
        }

        sqlx::query!(
            "UPDATE fetch_jobs SET status = 'succeeded', updated_at = now() WHERE id = $1",
            job_id
        )
        .execute(pool)
        .await?;
    }

    Ok(summary)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo test -p shovelsup-pipeline --lib worker`
Expected: `test worker::tests::run_due_fetch_jobs_skips_municipality_with_no_agenda_url ... ok`.

- [ ] **Step 5: Regenerate the SQLx offline query cache**

Run: `cd apps/web && DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo sqlx prepare --workspace`
Expected: exits 0, `.sqlx/` directory (at the workspace root, `apps/web/.sqlx/`) has new/updated `query-*.json` files for the new queries added in Tasks 3 and 5. If a large number of unrelated files change (not just ones matching this task's or Task 3's actual SQL text), that's a signal of a tool-version mismatch, not real drift — verify a sample of any changed file against the actual SQL text in the diff before assuming it's fine, don't just trust file-count or "renamed" labels.

- [ ] **Step 6: Commit**

```bash
git add apps/web/pipeline/src/worker.rs apps/web/.sqlx
git commit -m "feat(imp-req-001-11): implement the fetch-job worker's core discover/fetch/parse/extract loop"
```

---

### Task 6: `pipeline/tests/pipeline_worker.rs` — happy path and dedupe

**Files:**
- Create: `apps/web/pipeline/tests/pipeline_worker.rs`

**Interfaces:**
- Consumes: `shovelsup_pipeline::worker::run_due_fetch_jobs`, `shovelsup_pipeline::worker::WorkerSummary` (Task 5), `shovelsup_pipeline::extractor::llm::AnthropicProvider::from_env` (existing, real-API integration test convention already used by `pipeline/tests/pipeline_extraction.rs`), `shovelsup_pipeline::parser::ocr::TesseractOcrProvider`, `shovelsup_pipeline::fetcher::Fetcher` — NOTE: `WorkerSummary`'s fields and `run_due_fetch_jobs` must both be `pub` for this integration test (a separate crate-external binary) to use them; confirm Task 5 made them `pub`, not `pub(crate)`, before writing this test.

**Requires:** a real `ANTHROPIC_API_KEY` in the environment (same as existing `pipeline/tests/pipeline_extraction.rs` tests) — confirm one is available before starting; if not, stop and ask rather than substituting a fake key that will make these tests fail or hang.

- [ ] **Step 1: Write the failing test**

Create `apps/web/pipeline/tests/pipeline_worker.rs`:

```rust
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
```

Use whichever `migrations = "..."` relative path Task 3/5 established as working — copy it exactly rather than trust `"../web/migrations"` blindly.

- [ ] **Step 2: Run tests to verify they fail**

Run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup ANTHROPIC_API_KEY=<real key> cargo test -p shovelsup-pipeline --test pipeline_worker`
Expected: FAIL if `WorkerSummary`/`run_due_fetch_jobs` aren't `pub` (compile error), or an assertion failure — resolve either before proceeding to Step 3.

- [ ] **Step 3: Run against the real Anthropic API to confirm real extraction behavior**

Run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup ANTHROPIC_API_KEY=<real key> cargo test -p shovelsup-pipeline --test pipeline_worker -- --test-threads=1`
Expected: both tests pass. `REAL_MINUTES_TEXT` is a real, previously-fetched Montreal resolution (see `pipeline/tests/pipeline_extraction_fr.rs`'s `CM26 0046` fixture) that does **not** qualify (administrative land sale, no physical-work scale indicator) — so `documents_ingested` counts the document as fetched+parsed+extraction-attempted even though no `project_mentions` row results; the assertions above check `source_documents`/`fetch_jobs` state, not `project_mentions`, precisely because this real text is a non-qualifying case.

- [ ] **Step 4: Commit**

```bash
git add apps/web/pipeline/tests/pipeline_worker.rs
git commit -m "test(imp-req-001-11): worker end-to-end ingestion and dedupe against a mock listing page"
```

---

### Task 7: `pipeline/tests/pipeline_worker.rs` — failure handling

**Files:**
- Modify: `apps/web/pipeline/tests/pipeline_worker.rs`

**Interfaces:**
- Consumes: same as Task 6.

- [ ] **Step 1: Write the failing tests**

Append to `apps/web/pipeline/tests/pipeline_worker.rs`:

```rust
/// The listing page itself failing to fetch must fail the job with a
/// recorded error, not panic or silently no-op.
#[sqlx::test(migrations = "../web/migrations")]
async fn worker_marks_job_failed_when_listing_page_fetch_fails(pool: PgPool) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/listing"))
        .respond_with(ResponseTemplate::new(503))
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

    assert_eq!(summary.failed, 1);

    let job = sqlx::query!(
        "SELECT status, attempts, last_error FROM fetch_jobs WHERE municipality_id = $1",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.attempts, 1);
    assert!(job.last_error.is_some());
}

/// One discovered document failing to fetch must not block the others in
/// the same job — see the design doc's worker step 5 per-document isolation.
#[sqlx::test(migrations = "../web/migrations")]
async fn worker_isolates_one_bad_discovered_document_from_the_rest(pool: PgPool) {
    let server = MockServer::start().await;
    let listing_html = r#"<html><body>
<a href="/docs/good.pdf?typeDoc=pv&doc=1">Good</a>
<a href="/docs/bad.pdf?typeDoc=pv&doc=2">Bad</a>
</body></html>"#;
    Mock::given(method("GET"))
        .and(path("/listing"))
        .respond_with(ResponseTemplate::new(200).set_body_string(listing_html))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/docs/good.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string(REAL_MINUTES_TEXT),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/docs/bad.pdf"))
        .respond_with(ResponseTemplate::new(404))
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

    assert_eq!(summary.documents_ingested, 1, "the good document must still be ingested");
    assert_eq!(summary.failed, 1, "the bad document is counted as failed, not silently dropped");

    let job_status: String = sqlx::query_scalar!(
        "SELECT status FROM fetch_jobs WHERE municipality_id = $1",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        job_status, "succeeded",
        "one bad document must not fail the whole job"
    );
}
```

- [ ] **Step 2: Run tests**

Run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup ANTHROPIC_API_KEY=<real key> cargo test -p shovelsup-pipeline --test pipeline_worker -- --test-threads=1`
Expected: all 4 `pipeline_worker` tests pass (2 from Task 6, 2 new here).

- [ ] **Step 3: Commit**

```bash
git add apps/web/pipeline/tests/pipeline_worker.rs
git commit -m "test(imp-req-001-11): worker isolates listing-page and per-document failures"
```

---

### Task 8: Wire the interval loop into `main.rs`

**Files:**
- Modify: `apps/web/web/src/main.rs`

**Interfaces:**
- Consumes: `shovelsup_web::config::flags::data_pipeline_ingestion_enabled` (Task 2), `shovelsup_pipeline::scheduler::Scheduler::enqueue_due_fetches` (existing), `shovelsup_pipeline::worker::run_due_fetch_jobs` (Task 5), `shovelsup_pipeline::parser::ocr::TesseractOcrProvider` (existing), `shovelsup_pipeline::extractor::llm::AnthropicProvider::from_env` (existing).

- [ ] **Step 1: Add the spawn block**

`web/Cargo.toml` already depends on `shovelsup-pipeline` and `chrono` — no `Cargo.toml` change needed.

In `apps/web/web/src/main.rs`, add these imports at the top (alongside the existing `use` block):

```rust
use chrono::Utc;
use shovelsup_pipeline::extractor::llm::AnthropicProvider;
use shovelsup_pipeline::parser::ocr::TesseractOcrProvider;
use shovelsup_pipeline::scheduler::Scheduler;
use shovelsup_pipeline::worker;
use shovelsup_web::config::flags::data_pipeline_ingestion_enabled;
use std::time::Duration;
```

Then, after `let state = AppState { ... };` and before `let app = shovelsup_web::app(state)...`, insert:

```rust
    let pipeline_db = state.db.clone();
    tokio::spawn(async move {
        let ocr = TesseractOcrProvider;
        let llm = AnthropicProvider::from_env();
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if !data_pipeline_ingestion_enabled() {
                continue;
            }
            if let Err(e) = Scheduler::enqueue_due_fetches(&pipeline_db, Utc::now()).await {
                tracing::error!(error = %e, "enqueue_due_fetches failed");
            }
            match worker::run_due_fetch_jobs(&pipeline_db, &ocr, &llm).await {
                Ok(summary) => tracing::info!(?summary, "pipeline tick complete"),
                Err(e) => tracing::error!(error = %e, "run_due_fetch_jobs failed"),
            }
        }
    });
```

Note: `AnthropicProvider::from_env()` calls `.expect("ANTHROPIC_API_KEY must be set")` (existing behavior) — this executes once when the spawned task starts, at server startup, not per-tick, so a missing key fails fast at boot rather than silently on the first tick.

- [ ] **Step 2: Build**

Run (from `apps/web/`): `cargo build --workspace`
Expected: compiles with no errors or new warnings.

- [ ] **Step 3: Manual verification — confirm the loop runs and respects the flag**

Run: `docker compose up -d && DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup DATA_PIPELINE_INGESTION_ENABLED=false ANTHROPIC_API_KEY=<real key> cargo run -p shovelsup-web`
Expected: server starts and logs `listening on 0.0.0.0:3000`; no pipeline tick logs appear (flag is off). Stop with Ctrl-C.

Then run: `DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup DATA_PIPELINE_INGESTION_ENABLED=true ANTHROPIC_API_KEY=<real key> timeout 15 cargo run -p shovelsup-web`
Expected: `tokio::time::interval` first ticks after the full 3600s duration by default, so within 15s this only confirms startup behavior (no panic, process stays up), not a full tick.

- [ ] **Step 4: Commit**

```bash
git add apps/web/web/src/main.rs
git commit -m "feat(imp-req-001-12): spawn hourly interval loop for the fetch pipeline"
```

---

### Task 9: Documentation updates

**Files:**
- Modify: `docs/runbooks/data_pipeline_ingestion.md`
- Modify: `apps/web/IMPLEMENTATION_CHECKLIST.md`

- [ ] **Step 1: Update the runbook's "Current implementation status" section**

In `docs/runbooks/data_pipeline_ingestion.md`, replace the `## Current implementation status (as of REQ-001 Loop B)` section (everything from that heading through the end of the "Municipal calendar systems" table) with:

```markdown
## Current implementation status (as of the fetch-job worker, 2026-07-11)

`Fetcher` (allowlist enforcement, checksum dedupe, retry/backoff), `Scheduler`
(daily-fallback `fetch_jobs` enqueue), `worker::core::extract_pv_document_links`
(real `typeDoc=pv` link discovery from Montreal's document-listing page), and
`worker::run_due_fetch_jobs` (discover → fetch → parse → extract per pending
job) all live in the `shovelsup-pipeline` crate (`apps/web/pipeline/`) and are
implemented and tested. A `tokio::spawn` interval loop in the `shovelsup-web`
crate's `main.rs` calls `Scheduler` and the worker every hour, gated live by
this flag — see `docs/adr/006-tokio-interval-loop-for-pipeline-scheduling.md`.

Montreal is the only municipality with a configured `agenda_url`
(`apps/web/web/migrations/015_montreal_agenda_url.sql`) — Toronto and
Vancouver stay unconfigured (`agenda_url IS NULL`), and the worker skips them
rather than failing. See
`docs/superpowers/specs/2026-07-11-fetch-job-worker-design.md` for the full
design, including why Vancouver (HTTP 403 on every known path) and Toronto
are out of scope for this pass.

**Before enabling this flag in a real environment**: confirm with the user
whether legal/public-source review sign-off has actually happened — the
seeded domain allowlists (`002_seed_municipalities.sql`) were still marked
`ASSUMED pending legal review` as of that migration, with a target date of
2026-07-19 noted in an earlier version of this runbook.

### Municipal calendar systems (researched 2026-07-10, links verified 2026-07-11)

None of the three launch municipalities exposes an iCal/RSS/JSON feed of
council meetings. Montreal's real document index — reachable, static HTML,
no calendar/date computation needed — is at
`https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL`
(linked from the marketing page `montreal.ca/conseils-decisionnels/conseil-municipal`).

| Municipality | Calendar system | Machine-readable feed? | Confirmed document domains |
| --- | --- | --- | --- |
| Toronto | TMMIS (`app.toronto.ca/tmmis/`) | No | `toronto.ca`, `app.toronto.ca` |
| Vancouver | `covapp.vancouver.ca` interactive portal | No (the `opendata.vancouver.ca` minutes dataset only covers the 1970s, TXT format) | `vancouver.ca`, `covapp.vancouver.ca` |
| Montreal | `ville.montreal.qc.ca/portal/page?_pageid=5798,85945578...` (real, static HTML index, confirmed via direct fetch) | No (browsable index, not a feed) | `montreal.ca`, `ville.montreal.qc.ca`, `portail-m4s.s3.montreal.ca` (S3-backed asset host) |

The domain allowlists reflect the confirmed values above.
```

- [ ] **Step 2: Update the implementation checklist**

In `apps/web/IMPLEMENTATION_CHECKLIST.md`, under the `## REQ-001` section's task list (after the existing `IMP-REQ-001-10` line), add:

```markdown
- [x] IMP-REQ-001-11 — Fetch-job worker: discover real typeDoc=pv links from Montreal's document listing, fetch/parse/extract each (`pipeline/src/worker.rs`, `pipeline/src/worker/core.rs`)
- [x] IMP-REQ-001-12 — Wire hourly `tokio::spawn` interval loop in `web/src/main.rs`, gated by live-read `DATA_PIPELINE_INGESTION_ENABLED`
- [x] IMP-REQ-001-13 — `agenda_url` column + real Montreal seed (`web/migrations/015_montreal_agenda_url.sql`) ⚠️ Toronto/Vancouver stay unconfigured, see runbook
```

- [ ] **Step 3: Commit**

```bash
git add docs/runbooks/data_pipeline_ingestion.md apps/web/IMPLEMENTATION_CHECKLIST.md
git commit -m "docs(imp-req-001): update runbook and checklist for the fetch-job worker"
```

---

### Task 10: Full verification pass

**Files:** none (verification only)

- [ ] **Step 1: Run the full test suite**

Run: `cd apps/web && DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup ANTHROPIC_API_KEY=<real key> cargo test --workspace -- --test-threads=1`
Expected: all tests pass, including every test added in Tasks 3–7 alongside the full pre-existing suite (confirms no regression).

- [ ] **Step 2: Confirm the SQLx offline cache is committed and consistent**

Run: `cd apps/web && DATABASE_URL=postgres://shovelsup:change-me@localhost:5434/shovelsup cargo sqlx prepare --workspace --check`
Expected: exits 0 — no drift between `.sqlx/` and the queries actually in source. If this fails purely due to a local sqlx-cli version mismatch (see Task 5 Step 5's note), report that distinction explicitly rather than silently re-running prepare and committing whatever it produces.

- [ ] **Step 3: Re-run the `architecture` skill to refresh C4 diagrams**

Invoke the `architecture` skill (per the project's living-documentation requirement) to regenerate `docs/architecture/` diagrams reflecting the new `worker` module (and its `core` submodule), the interval-loop runtime element, and the current three-crate workspace shape.

- [ ] **Step 4: Final review**

Confirm with the user before setting `DATA_PIPELINE_INGESTION_ENABLED=true` in any real (non-local) environment — per Task 9's runbook update, this depends on legal/public-source sign-off status that this plan cannot verify on its own.
