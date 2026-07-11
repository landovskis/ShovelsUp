# ADR 006 — In-Process Tokio Interval Loop for Pipeline Scheduling

**Status**: Accepted
**Date**: 2026-07-11
**Feature**: specs/2026-07-11-fetch-job-worker-design

## Context

REQ-001's data pipeline (`Scheduler::enqueue_due_fetches`, `Fetcher`, `parse_and_store`,
`extract_and_store`) is fully implemented and unit/integration tested, but nothing in
`src/main.rs` ever calls any of it in production — the Axum server starts and serves
HTTP, full stop. `src/jobs/sla_sweep.rs` independently confirms the same gap for a
different job: "this codebase has no periodic-execution infra anywhere yet (checked:
`main.rs` has no `tokio::spawn`/interval loop at all)."

This is the first time the codebase needs *any* periodic execution, so the choice made
here sets the pattern for the SLA sweep and public-search-refresh jobs too.

Options considered:

| Option | Description |
|--------|-------------|
| In-process `tokio::spawn` + `tokio::time::interval` | A background task inside the same process as the web server, spawned at startup |
| External cron / k8s CronJob invoking a one-shot binary/CLI subcommand | A separate scheduled invocation outside the running server process |
| Dedicated job-queue system (e.g. a Rust `sidekiq`-alike, or an external queue like SQS) | Durable, distributed job scheduling with its own worker pool |

## Decision

Use an **in-process `tokio::spawn` task with `tokio::time::interval`**, started in
`src/main.rs` alongside the Axum server, ticking hourly.

Each tick: check `DATA_PIPELINE_INGESTION_ENABLED` (read live from the environment,
not cached), call `Scheduler::enqueue_due_fetches`, then
`worker::run_due_fetch_jobs`. Every fallible step inside the loop body is matched and
logged via `tracing::error!` — never `.unwrap()`'d or propagated with `?` — so a
transient failure (a bad HTTP response, a single malformed document) can never take
down the loop or the server process.

## Rationale

- The app runs as a single Docker Compose service today (ADR 005) — there is no
  multi-instance deployment to coordinate across, so distributed locking or a queue
  system solves a problem this deployment doesn't have.
- An external cron/CronJob would require a separate binary entrypoint, a separate
  deployment artifact, and duplicate startup logic (DB pool, Redis, env parsing) that
  `main.rs` already does — pure overhead for a single-process app.
- `tokio::spawn` + `interval` is the smallest mechanism that satisfies "run this
  periodically without a restart," reuses the `AppState`'s existing `PgPool`
  connection, and requires no new infrastructure or dependencies (tokio's `time`
  feature is already in use via `tokio::time::sleep` in `Fetcher::fetch_with_retry`).

## Consequences

- **Ties execution to the web server's uptime.** If the process restarts or crashes,
  the interval loop restarts with it — there is no execution history outside this
  process. This is acceptable at current scale (single instance, hourly cadence,
  `fetch_jobs.status` already durable in Postgres so a missed tick just means the job
  runs on the next one).
- **No cross-instance coordination exists.** If this app is ever horizontally scaled
  to multiple instances, every instance will run its own interval loop and race to
  claim the same `fetch_jobs` rows. This ADR explicitly defers that problem — revisit
  before scaling beyond one instance (e.g. `SELECT ... FOR UPDATE SKIP LOCKED`, or
  moving to an external scheduler).
  Precedent for future jobs (`sla_sweep`, `public_search_refresh`) —
  the same interval-loop pattern should be reused for those rather than inventing a
  second mechanism, unless multi-instance scaling happens first, in which case this
  ADR should be revisited before adding more jobs to it.
