# Runbook: data pipeline ingestion

## What runs

The full fetch → parse → extract → normalize → resolve pipeline (REQ-001
through REQ-007) runs on an hourly in-process interval.

## Production prerequisite

Do **not** enable in production until the municipal domain allowlist in
`migrations/002_seed_municipalities.sql` has legal/public-source review
sign-off — the seeded Toronto/Vancouver/Montreal domains are marked
`ASSUMED pending legal review` (see Implementation Plan REQ-001 risk,
target 2026-07-19).

There is no runtime feature switch. Deploy the application only after the
legal/public-source review is complete. To stop ingestion after deployment,
roll back or stop the web process; existing data does not require cleanup.

## Current implementation status (as of the fetch-job worker, 2026-07-11)

`Fetcher` (allowlist enforcement, checksum dedupe, retry/backoff), `Scheduler`
(daily-fallback `fetch_jobs` enqueue), `worker::core::extract_pv_document_links`
(real `typeDoc=pv` link discovery from Montreal's document-listing page), and
`worker::run_due_fetch_jobs` (discover → fetch → parse → extract per pending
job) all live in the `shovelsup-pipeline` crate (`apps/web/pipeline/`) and are
implemented and tested. A `tokio::spawn` interval loop in the `shovelsup-web`
crate's `main.rs` calls `Scheduler` and the worker every hour — see
`docs/adr/006-tokio-interval-loop-for-pipeline-scheduling.md`.

Montreal is the only municipality with a configured `agenda_url`
(`apps/web/web/migrations/015_montreal_agenda_url.sql`) — Toronto and
Vancouver stay unconfigured (`agenda_url IS NULL`), and the worker skips them
rather than failing. See
`docs/superpowers/specs/2026-07-11-fetch-job-worker-design.md` for the full
design, including why Vancouver (HTTP 403 on every known path) and Toronto
are out of scope for this pass.

**Before deploying in a real environment**: confirm with the user
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
