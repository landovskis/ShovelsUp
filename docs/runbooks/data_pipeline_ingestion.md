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
2. Restart the app so migrations/config are picked up.

## Rollback

All migrations under this flag are additive-only (no destructive schema
changes). To roll back:

1. Set `DATA_PIPELINE_INGESTION_ENABLED=false` and restart.
2. No data cleanup is required — existing `source_documents`/`fetch_jobs`
   rows are inert once the flag is off.

## Current implementation status (as of REQ-001 Loop B)

`Fetcher` (allowlist enforcement, checksum dedupe, retry/backoff) and
`Scheduler` (daily-fallback `fetch_jobs` enqueue) are implemented and
tested, but **nothing yet consumes `fetch_jobs` and invokes `Fetcher`** —
the Implementation Plan's REQ-001 task list has no "worker" task wiring the
two together, and `fetch_jobs` has no `source_url` column, since resolving
which URL a given meeting's minutes live at requires real per-municipality
calendar integration not specified in the PRD (see the Scheduler module doc
comment). Until that worker and URL-resolution strategy are added, this
flag does not yet gate any live fetching — it is documented ahead of that
work per the task's own scope (docs-only, validated by manual review).

### Municipal calendar systems (researched 2026-07-10)

None of the three launch municipalities exposes an iCal/RSS/JSON feed of
council meetings — resolving a `source_url` per meeting means scraping an
HTML calendar page for each city. This is real scope for a future task, not
something this run fabricated a stand-in for.

| Municipality | Calendar system | Machine-readable feed? | Confirmed document domains |
| --- | --- | --- | --- |
| Toronto | TMMIS (`app.toronto.ca/tmmis/`) | No | `toronto.ca`, `app.toronto.ca` |
| Vancouver | `covapp.vancouver.ca` interactive portal | No (the `opendata.vancouver.ca` minutes dataset only covers the 1970s, TXT format) | `vancouver.ca`, `covapp.vancouver.ca` |
| Montreal | `montreal.ca/conseils-decisionnels/conseil-municipal`; a session-*date* dataset exists at `donnees.montreal.ca` but not one for agenda/minutes documents | No (dates only) | `montreal.ca`, `ville.montreal.qc.ca`, `portail-m4s.s3.montreal.ca` (S3-backed asset host) |

Migration `004_expand_municipality_allowlists.sql` updates the domain
allowlists to the confirmed values above — the original placeholder
(`{municipality}.ca` only) would have rejected real agenda/minutes URLs on
Toronto and Montreal, both of which serve documents from a different
domain than the city's main site.
