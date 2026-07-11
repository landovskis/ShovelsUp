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
cannot do calendar-aware discovery of "this meeting's documents." The product is
Montreal-first (`CLAUDE.md`), and Montreal's real council-meetings listing page —
`https://montreal.ca/conseils-decisionnels/conseil-municipal` — is reachable and already
covered by the domain allowlist (`montreal.ca`, expanded in
`migrations/004_expand_municipality_allowlists.sql`). Toronto and Vancouver are out of
scope for this pass (Vancouver returns HTTP 403 on every known path; see
`apps/web/tests/pipeline_extraction.rs` header).

## Goal

Make the pipeline actually run end-to-end for Montreal on a recurring schedule, without
inventing calendar-discovery infrastructure the PRD doesn't specify. Everything else
(Toronto/Vancouver `agenda_url`, calendar polling, automatic per-meeting PDF discovery)
stays a documented gap, consistent with the scope `Scheduler` already committed to.

## Non-goals

- Discovering individual meeting-minute PDF URLs automatically (e.g. parsing the
  Montreal listing page for links, or constructing dated URLs). The worker fetches
  whatever is at `agenda_url` and relies on existing checksum dedupe
  (`Fetcher::fetch`) to avoid reprocessing unchanged content.
- Toronto/Vancouver `agenda_url` values.
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
SET agenda_url = 'https://montreal.ca/conseils-decisionnels/conseil-municipal'
WHERE slug = 'montreal';
```

Toronto/Vancouver rows keep `agenda_url = NULL`.

## Components

### 1. `src/pipeline/worker.rs` (new)

```rust
pub struct WorkerSummary {
    pub succeeded: usize,
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
3. Call `Fetcher::fetch(pool, municipality_id, agenda_url)`.
   - `Err(FetchError)` → job `failed`, `attempts += 1`, `last_error` set to the
     error's `Display` output.
   - `Ok(Duplicate { .. })` → job `succeeded`, no further work (content unchanged
     since last fetch).
   - `Ok(Fetched { document_id })` → continue to step 4.
4. `parse_and_store(pool, document_id, ocr)`. A parse failure already records itself
   on `source_documents.parser_status` (`failed`/`reprocessing`) per its own doc
   comment; the job is still marked `succeeded` at the job level — fetching succeeded,
   and parsing has its own status/retry story via the existing admin reprocess
   endpoint for source documents.
5. Query `document_chunks` for `source_document_id = document_id`, and call
   `extract_and_store(pool, chunk.id, &chunk.content, llm)` for each. Only a
   `sqlx::Error` here propagates as `Err` from the worker (per `extract_and_store`'s
   own contract) — everything else is already a handled, recorded outcome.
6. Mark the job `succeeded`.

Any unexpected `sqlx::Error` (not a handled per-job outcome) bubbles up from
`run_due_fetch_jobs` itself and is logged by the caller — it means something is wrong
with the database connection, not with one job.

### 2. Wiring into `src/main.rs`

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

- **Unit** (`src/pipeline/worker.rs` `#[cfg(test)]`): a municipality with `agenda_url =
  NULL` is skipped (counted, job stays `pending`), not marked `failed`.
- **Integration** (`tests/pipeline_worker.rs`, `sqlx::test` + `wiremock`, same shape as
  `tests/pipeline_fetch.rs`):
  - A pending job for a municipality with a reachable mock `agenda_url` ends with a
    `project_mentions` row present, using a fake `LlmProvider` (existing test double
    pattern from `tests/pipeline_extraction.rs`).
  - `DATA_PIPELINE_INGESTION_ENABLED` unset/`false` → calling the tick logic is a
    no-op (tested at the `ingestion_enabled()` + loop-body level, not by spinning up
    the real `tokio::spawn` loop).
  - A fetch that errors (mock server returns 500 past retry budget) → job `failed`,
    `last_error` populated, `attempts` incremented.
  - A duplicate fetch (identical checksum) → job `succeeded`, no new `document_chunks`
    or `project_mentions` created.
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
