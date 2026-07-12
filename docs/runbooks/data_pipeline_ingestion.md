# Runbook: `DATA_PIPELINE_INGESTION_ENABLED`

## What it gates

The full fetch → parse → extract → normalize → resolve pipeline (REQ-001
through REQ-007). Default: `false`.

## Enabling in production

Do **not** enable in production until the municipal domain allowlist in
`migrations/002_seed_municipalities.sql` has legal/public-source review
sign-off — the seeded Toronto/Vancouver/Montreal domains are marked
`ASSUMED pending legal review` (see Implementation Plan REQ-001 risk,
target 2026-07-19).

Once cleared:

1. Set `DATA_PIPELINE_INGESTION_ENABLED=true` in the target environment.
2. The change takes effect on the next hourly tick — the flag is read live
   (not cached) by the `tokio::spawn` interval loop in `main.rs`, so no
   restart is required. If you also want it to take effect before the next
   scheduled tick, there is currently no way to force an out-of-schedule
   tick — this is a known limitation.

## Rollback

All migrations under this flag are additive-only (no destructive schema
changes). To roll back:

1. Set `DATA_PIPELINE_INGESTION_ENABLED=false`.
2. The change takes effect on the next hourly tick, same as enabling — no
   restart is required (see "Enabling in production" above).
3. No data cleanup is required — existing `source_documents`/`fetch_jobs`
   rows are inert once the flag is off.

## Recovering a stuck job

If a `fetch_jobs` row is stuck in `in_progress` (e.g. after a process crash
mid-job, or a genuine `sqlx::Error` propagating out of `parse_and_store`/
`extract_and_store` mid-job), it will never be automatically reclaimed — the
worker (`worker::run_due_fetch_jobs`) only ever selects `status = 'pending'`
rows, so a stuck `in_progress` row is silently skipped forever. It also
blocks a fresh job for that municipality for the rest of the day, since the
scheduler's daily dedup only checks whether a row already exists for that
municipality/day, not its status.

**Known gap:** the existing admin endpoint `POST
/admin/fetch_jobs/{id}/reprocess` (`apps/web/web/src/routes/admin.rs`,
`reprocess_fetch_job`) **cannot** currently be used to reset a stuck
`in_progress` job — it explicitly returns `409 Conflict` when the job's
status is already `pending` or `in_progress` ("nothing to reprocess"), by
design, to avoid interfering with a job that's genuinely still running.
There is currently no supported way to reset a stuck `in_progress` job back
to `pending` short of a manual `UPDATE fetch_jobs SET status = 'pending',
attempts = 0, last_error = NULL, updated_at = now() WHERE id = '<id>'` run
directly against the database. Building a safe, automatic (or admin-endpoint)
reclaim path for stuck `in_progress` jobs is out of scope for this pass and
should be tracked as follow-up work.

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

### Known limitation: re-published documents are not re-ingested

`worker::run_due_fetch_jobs`'s discovery-skip stage dedupes each discovered
document link against `source_documents` by `(municipality_id, source_url)`,
not by content checksum. If a municipality re-publishes a document with
amended content at the same URL, it will be skipped as already-ingested and
the amended content will never be picked up. This is a known, accepted
limitation, not a bug to fix.
