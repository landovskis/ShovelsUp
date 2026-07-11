# Design: Wire the Data Pipeline End-to-End (Fetch-Job Worker + Periodic Scheduling)

**Date**: 2026-07-11
**Related**: REQ-001 (Automatically Fetch Proceedings), `docs/runbooks/data_pipeline_ingestion.md`, `docs/adr/006-tokio-interval-loop-for-pipeline-scheduling.md`

## Problem

The data pipeline never runs in production. Every stage (`Fetcher`, `parse_and_store`,
`extract_and_store` — which already chains normalize/conflict-detection/resolve
internally) is implemented and tested in isolation, but nothing connects them:

- `Scheduler::enqueue_due_fetches` only inserts `fetch_jobs` rows (`status = 'pending'`).
  Nothing reads pending rows and calls `Fetcher::fetch` on them.
- `municipalities` has no persisted fetch URL. `fetch_jobs` has no `source_url` column
  either. This is a known, documented gap (`src/pipeline/scheduler.rs` doc comment;
  `docs/runbooks/data_pipeline_ingestion.md` "Current implementation status").
- `src/main.rs` starts the Axum HTTP server only. There is no `tokio::spawn`, interval
  loop, or any periodic-execution mechanism anywhere in the codebase
  (`src/jobs/sla_sweep.rs:4-8` notes this explicitly for the same reason).
- `DATA_PIPELINE_INGESTION_ENABLED` is documented in the runbook and present in
  `.env.example`, but no code reads it — it gates nothing today.

Confirmed via `docs/runbooks/data_pipeline_ingestion.md`'s own research (2026-07-10):
none of the three launch municipalities expose a machine-readable calendar feed, so V1
cannot do calendar-aware discovery of "the next meeting's documents" ahead of time. The
product is Montreal-first (`CLAUDE.md`).

`https://montreal.ca/conseils-decisionnels/conseil-municipal` (the page originally
proposed as `agenda_url`) turned out to be a marketing/schedule page that itself links
out to the real document index — verified directly, not assumed:

- `https://montreal.ca/conseils-decisionnels/conseil-municipal` → links to
  `https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL`.
- That portal page is static HTML (no JavaScript rendering required — confirmed by
  fetching it directly with `curl`) containing real per-meeting document links in the
  form `/sel/adi-public/afficherpdf/fichier.pdf?typeDoc={odj|pv|da}&doc={id}`:
  `odj` = ordre du jour (agenda, pre-decision, no recorded vote outcome), `pv` =
  procès-verbal (minutes, has the "Adopté à l'unanimité"-style decision text this
  product needs), `da` = attachment. The host, `ville.montreal.qc.ca`, is already
  allowlisted (`migrations/004_expand_municipality_allowlists.sql`).
- The page listed the full 2026 meeting history at fetch time (January through June),
  not just the latest meeting.

Toronto and Vancouver are out of scope for this pass (Vancouver returns HTTP 403 on
every known path; see `apps/web/tests/pipeline_extraction.rs` header).

## Goal

Make the pipeline actually run end-to-end for Montreal on a recurring schedule:
discover real `typeDoc=pv` (minutes) document links from the real listing page, fetch
only ones not already ingested, and run each through parse → extract. Everything else
(Toronto/Vancouver `agenda_url`, calendar-aware "next meeting" polling ahead of
publication) stays a documented gap, consistent with the scope `Scheduler` already
committed to.

## Non-goals

- Toronto/Vancouver `agenda_url` values.
- Fetching `odj` (agenda) or `da` (attachment) links — only `pv` (minutes) documents
  carry the recorded decision text (`approval_status_raw`) this product surfaces.
- Calendar-aware "poll ahead of the next meeting" scheduling — the worker only
  discovers documents already published on the listing page at tick time.
- Retry/backoff at the `fetch_jobs` level beyond what already exists inside `Fetcher`
  (HTTP-level retry) — a failed job is marked `failed` and left for the existing admin
  reprocess endpoint (`POST /admin/fetch_jobs/{id}/reprocess`), not auto-retried.
- Distributed/multi-instance locking for the interval loop. The app runs as a single
  process today (see ADR 005's Docker Compose setup); adding a lock is unnecessary
  complexity until there's more than one instance.

## Data model change

One additive migration:

```sql
ALTER TABLE municipalities ADD COLUMN agenda_url TEXT;

UPDATE municipalities
SET agenda_url = 'https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL'
WHERE slug = 'montreal';
```

`agenda_url` here means "the document index page to discover links from," not "the one
document to fetch" — the worker treats it differently for Montreal (discovery) than a
future municipality with a direct single-document URL might need, but the column stores
the same thing either way: where the worker starts.

Toronto/Vancouver rows keep `agenda_url = NULL`.

No new table for tracking discovered documents: `source_documents.source_url` is
already stored by `Fetcher::fetch` on every persisted fetch. "Already discovered" is
answered by `SELECT 1 FROM source_documents WHERE municipality_id = $1 AND source_url =
$2` before issuing the GET for a discovered link — reuses existing state instead of
adding a parallel tracking mechanism.

## Components

### 1. `src/pipeline/discovery.rs` (new)

A pure, unit-testable link-extraction function — no I/O:

```rust
/// Extracts absolute URLs for `typeDoc=pv` (procès-verbal/minutes) links from
/// `html`, resolving relative hrefs against `base_url`. Ignores `odj` (agenda)
/// and `da` (attachment) links — see the design doc's Non-goals.
pub fn extract_pv_document_links(html: &str, base_url: &str) -> Vec<String>
```

Implementation: a regex over `href="([^"]*fichier\.pdf\?[^"]*typeDoc=pv[^"]*)"`
(case-insensitive), each match resolved against `base_url` via `reqwest::Url::join`.
Tested against a real captured HTML fixture (the portal page fetched during this
design's research), not a hand-written stand-in, so the test would actually catch a
real markup change.

### 2. `Fetcher::fetch_bytes` (new method on the existing `Fetcher`, `src/pipeline/fetcher.rs`)

The listing/index page itself is not a decision-bearing document — it should not be
persisted to `source_documents` or run through parse/extract, only used transiently to
discover links. But it still needs the same allowlist enforcement and retry/backoff
`Fetcher::fetch` already has, so refactor rather than duplicate:

```rust
pub async fn fetch_bytes(
    &self,
    pool: &PgPool,
    municipality_id: Uuid,
    url: &str,
) -> Result<Vec<u8>, FetchError>
```

Extracts the shared "parse URL → check allowlist → `fetch_with_retry`" prefix that
`fetch()` already has into this method; `fetch()` calls it and then adds the
checksum/persist step on top, instead of duplicating the allowlist+retry logic.

### 3. `src/pipeline/worker.rs` (new)

```rust
pub struct WorkerSummary {
    pub documents_ingested: usize,
    pub documents_skipped_duplicate: usize,
    pub failed: usize,
    pub skipped_no_agenda_url: usize,
}

pub async fn run_due_fetch_jobs(
    pool: &PgPool,
    ocr: &dyn OcrProvider,
    llm: &dyn LlmProvider,
) -> Result<WorkerSummary, sqlx::Error>
```

Per pending, due job (`status = 'pending' AND scheduled_for <= now()`):

1. Look up the municipality's `agenda_url`. `NULL` → leave the job `pending` (not
   `failed` — there's nothing wrong with the job, the municipality just isn't
   configured yet) and count it under `skipped_no_agenda_url`.
2. Mark the job `in_progress`.
3. `Fetcher::fetch_bytes(pool, municipality_id, agenda_url)` to get the listing page's
   HTML. `Err(FetchError)` → job `failed`, `attempts += 1`, `last_error` set, stop here
   — the index page itself couldn't be read, so there's nothing to discover.
4. `discovery::extract_pv_document_links(&html, agenda_url)` → a list of absolute
   minutes-document URLs.
5. For each discovered URL: skip if `SELECT 1 FROM source_documents WHERE
   municipality_id = $1 AND source_url = $2` already returns a row (counted under
   `documents_skipped_duplicate`). Otherwise call `Fetcher::fetch(pool,
   municipality_id, &url)`:
   - `Err(FetchError)` → does not fail the whole job; logged and counted under
     `failed`, the job continues to the next discovered URL (one bad link shouldn't
     block the rest of the meeting's documents).
   - `Ok(Duplicate { .. })` → shouldn't normally happen given the pre-check in this
     step, but handled the same as "nothing new to do" if it does (e.g. a race with
     another process).
   - `Ok(Fetched { document_id })` → `parse_and_store(pool, document_id, ocr)`, then
     query `document_chunks` for that document and call `extract_and_store(pool,
     chunk.id, &chunk.content, llm)` per chunk — same as before. Count under
     `documents_ingested`.
6. Mark the job `succeeded` once every discovered URL has been attempted (partial
   per-document failures don't fail the job — see step 5).

Any unexpected `sqlx::Error` (not a handled per-job/per-document outcome) bubbles up
from `run_due_fetch_jobs` itself and is logged by the caller — it means something is
wrong with the database connection, not with one job or one document.

### 4. Wiring into `src/main.rs`

```rust
let ocr = TesseractOcrProvider;
let llm = AnthropicProvider::from_env(); // concrete impl of the `LlmProvider` trait
let db_for_pipeline = state.db.clone();
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    loop {
        interval.tick().await;
        if !ingestion_enabled() {
            continue;
        }
        if let Err(e) = Scheduler::enqueue_due_fetches(&db_for_pipeline, Utc::now()).await {
            tracing::error!(error = %e, "enqueue_due_fetches failed");
        }
        match worker::run_due_fetch_jobs(&db_for_pipeline, &ocr, &llm).await {
            Ok(summary) => tracing::info!(?summary, "pipeline tick complete"),
            Err(e) => tracing::error!(error = %e, "run_due_fetch_jobs failed"),
        }
    }
});
```

`ingestion_enabled()` reads `DATA_PIPELINE_INGESTION_ENABLED` from the environment on
every call (not cached at startup), so ops can flip it without restarting — matching
the runbook's existing rollback story, but making it a live check instead of a
restart-required one. Read via `std::env::var(...).map(|v| v == "true").unwrap_or(false)`,
defaulting closed (matches `.env.example`'s documented default).

The spawned task never panics the process: every fallible call inside the loop is
matched and logged, never `.unwrap()`'d or `?`'d out of the loop body.

Interval: 1 hour, matching the "intended to run hourly" language already used for the
SLA sweep job precedent (`src/jobs/sla_sweep.rs`) and the Scheduler's own daily-fallback
cadence (checking hourly is frequent enough to catch a due daily job promptly without
being wasteful).

## Testing plan

- **Unit** (`src/pipeline/discovery.rs` `#[cfg(test)]`): `extract_pv_document_links`
  against the real captured HTML fixture returns exactly the known `typeDoc=pv` URLs
  from that fixture, and none of the `odj`/`da` ones — a fixed, checked-in fixture
  file, not a hand-rolled stand-in, so it exercises the real markup shape.
- **Unit** (`src/pipeline/worker.rs` `#[cfg(test)]`): a municipality with `agenda_url =
  NULL` is skipped (counted, job stays `pending`), not marked `failed`.
- **Integration** (`tests/pipeline_worker.rs`, `sqlx::test` + `wiremock`, same shape as
  `tests/pipeline_fetch.rs`):
  - A pending job for a municipality with a mock listing page (serving the same
    fixture HTML, with `href`s rewritten to the mock server) ends with
    `project_mentions` row(s) present for the discovered documents, using a fake
    `LlmProvider` (existing test double pattern from `tests/pipeline_extraction.rs`).
  - Running the same job a second time discovers the same links but fetches none of
    them again (all already have a matching `source_documents.source_url`) —
    `documents_skipped_duplicate` reflects this, no new chunks/mentions created.
  - `DATA_PIPELINE_INGESTION_ENABLED` unset/`false` → calling the tick logic is a
    no-op (tested at the `ingestion_enabled()` + loop-body level, not by spinning up
    the real `tokio::spawn` loop).
  - The listing page itself fails to fetch (mock server returns 500 past retry
    budget) → job `failed`, `last_error` populated, `attempts` incremented.
  - One discovered document fails to fetch while others succeed → job still
    `succeeded`, the failing document counted under `failed`, the rest ingested
    normally (step 5's per-document isolation).
- **E2E**: covered by the existing `TC-REQ-001-*` integration tests once wired to real
  `fetch_jobs` rows instead of calling `Fetcher` directly — no new E2E test type needed,
  but `IMPLEMENTATION_CHECKLIST.md` REQ-001 should be updated to reflect the worker now
  existing (was previously flagged as a gap in IMP-REQ-001-05's note and the runbook).

## Documentation to update as part of this work

- `docs/runbooks/data_pipeline_ingestion.md` — "Current implementation status" section
  is now stale (worker exists); update it, and confirm with the user before enabling
  the flag in any real environment given the runbook's legal-sign-off language.
- `apps/web/IMPLEMENTATION_CHECKLIST.md` — add `IMP-REQ-001-11` (worker) and
  `IMP-REQ-001-12` (periodic wiring in `main.rs`) under REQ-001.
- C4 container/component diagrams (`docs/architecture/`) — the worker and its interval
  loop are a new runtime component; re-run the `architecture` skill after
  implementation per the living-documentation requirement.
- ADR 006 recording the tokio-interval-loop decision (see companion file).
