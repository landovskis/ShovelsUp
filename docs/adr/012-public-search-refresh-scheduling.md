# ADR 012 — Public Search Index Refresh Runs in the Existing Pipeline Tick

**Status**: Accepted
**Date**: 2026-07-24
**Feature**: Implementation Plan: Public Discovery & Search (post-implementation follow-up)

## Context

`refresh_public_search_index` (`web/src/jobs/public_search_refresh.rs`) materializes
`public_search_documents` — the denormalized read-model every public search/results-list
query reads (REQ-004, REQ-008, REQ-009, REQ-015) — from `projects`/`project_mentions`/
`document_chunks`/`project_timeline_events`. It was implemented with its own thorough test
suite throughout REQ-004–015, but was never actually invoked anywhere in `main.rs`. A
manual browser-testing session after the plan's implementation completed found the public
search index empty in a real running server: no code path outside this job's own tests
ever called it, so nothing a resident/journalist searched for would ever appear, no matter
how much data the ingestion pipeline produced.

`main.rs` already runs one periodic job: a `tokio::spawn`'d `tokio::time::interval(3600s)`
loop (ADR 006) that calls `Scheduler::enqueue_due_fetches` then `worker::run_due_fetch_jobs`
once per hour, fires its first tick immediately on process start (Tokio's `interval()`
semantics), and logs a summary via `tracing`.

Options considered:

| Option | Description |
|--------|-------------|
| Call `refresh_public_search_index` inside the existing hourly tick, right after the fetch/extraction pipeline runs | No new interval, no new spawned task; refresh happens exactly when new/updated projects are most likely to exist |
| Spawn a second, independent `tokio::interval` loop just for this job | Lets the two run on different schedules, but doubles the amount of tick-loop boilerplate for no expressed need to decouple their timing |
| Trigger refresh synchronously from the resolver/pipeline write path, per project, instead of periodically | Lower staleness, but a much larger change (touches every write path that can create/update a `projects` row) than this follow-up asked for |
| Expose an admin-triggered manual refresh endpoint instead of/alongside a periodic job | Useful operationally, but doesn't solve "nothing appears by default" and wasn't asked for |

## Decision

Call `refresh_public_search_index(&pipeline_db)` inside the existing hourly tick loop in
`main.rs`, immediately after `worker::run_due_fetch_jobs` completes (success or failure —
a failed fetch tick doesn't skip refreshing whatever already exists), logging
`rows`/`error` via the same `tracing` pattern the fetch job already uses. No new spawned
task, no new interval, no new dependency.

## Rationale

- Reusing the existing tick loop is the smallest change that fixes the actual gap
  (nothing was ever calling this job in production) without introducing a second
  concurrent periodic task whose timing relationship to the first was never specified.
- Running refresh right after the fetch/extraction step, in the same tick, means new data
  the pipeline just produced is reflected in search results within the same cycle, not a
  cycle later.
- A synchronous per-write trigger was rejected as disproportionate: this follow-up's scope
  is "make the existing job actually run," not "redesign when re-materialization happens."
  If staleness becomes a real product problem, that's a distinct, separate decision.

## Consequences

- **Public search freshness is now bounded by the same hourly cadence as document
  ingestion** — a newly-resolved project can take up to an hour (typically much less,
  since the first tick fires on process start) to appear in search results, matching the
  ingestion pipeline's own existing freshness bound exactly.
- **A failure in `refresh_public_search_index` is logged but does not crash the process**
  or block the next tick, consistent with how `enqueue_due_fetches`/`run_due_fetch_jobs`
  failures are already handled — the public search index can lag behind `projects` if this
  job errors repeatedly, with no alerting beyond the log line today.
- **No change to any test** — `refresh_public_search_index`'s own test suite already
  covers its correctness; this ADR only concerns *when* it runs in production, which
  `main.rs` is not practical to unit-test directly (consistent with ADR 006's own
  precedent).
