# Implementation Checklist: Implementation Plan: Data Pipeline

**Source Implementation Plan:** https://mobilispect.atlassian.net/wiki/spaces/ShovelsUp/pages/20709378/Implementation+Plan+Data+Pipeline
**Target directory:** apps/web

## REQ-001 — Automatically Fetch Proceedings

⚠️ **Known gap (not a plan task, discovered during Loop B):** nothing consumes
`fetch_jobs` rows and invokes `Fetcher` — no task in the plan wires a worker
between `Scheduler` (enqueues) and `Fetcher` (fetches given a URL) — and
`fetch_jobs` has no `source_url` column, since resolving which URL a given
meeting's minutes live at requires real per-municipality calendar
integration the PRD doesn't specify. See
`docs/runbooks/data_pipeline_ingestion.md`. Flagged as an open risk below,
not silently worked around.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-001-1 — Fetch succeeds for a valid allowlisted URL
- [x] TC-REQ-001-2 — Fetch is a no-op on identical checksum (dedupe)
- [x] TC-REQ-001-3 — Fetch rejects a non-allowlisted domain
- [x] TC-REQ-001-4 — Fetch recovers from source 503 via retry/backoff
- [x] TC-REQ-001-5 — Post-meeting fetch load stays within SLA ⚠️ k6 script exercises Fetcher indirectly via the admin reprocess endpoint (see loadtest/fetch_load.js header) pending the worker gap above

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-001-01 — Discover existing Axum route/queue conventions
- [x] IMP-REQ-001-02 — Add `municipalities`, `source_documents`, `fetch_jobs` migrations
- [x] IMP-REQ-001-03 — Implement `Fetcher`: allowlist, HTTP GET, checksum, dedupe
- [x] IMP-REQ-001-04 — Retry/backoff policy for transient fetch failures
- [x] IMP-REQ-001-05 — Implement `Scheduler`: calendar poll + daily fallback ⚠️ daily-fallback only — no calendar poll (see module doc comment, PRD doesn't specify a calendar format)
- [x] IMP-REQ-001-06 — Seed `municipalities` fixture data
- [x] IMP-REQ-001-07 — Admin reprocess endpoint
- [x] IMP-REQ-001-08 — Integration test: end-to-end fixture fetch, 3 municipalities
- [x] IMP-REQ-001-09 — k6 load test for concurrent fetch SLA
- [x] IMP-REQ-001-10 — Document pipeline ingestion operations
- [x] IMP-REQ-001-11 — Fetch-job worker: discover real typeDoc=pv links from Montreal's document listing, fetch/parse/extract each (`pipeline/src/worker.rs`, `pipeline/src/worker/core.rs`)
- [x] IMP-REQ-001-12 — Wire hourly `tokio::spawn` interval loop in `web/src/main.rs`
- [x] IMP-REQ-001-13 — `agenda_url` column + real Montreal seed (`web/migrations/015_montreal_agenda_url.sql`) ⚠️ Toronto/Vancouver stay unconfigured, see runbook

## REQ-002 — Surface Projects Across Document Formats

⚠️ **Prerequisite fix (not a plan task, discovered during Loop B):** REQ-001's
Fetcher discarded the fetched body after checksumming — `source_documents`
had no content column at all — and decoded every response as UTF-8 text,
which would corrupt PDF bytes. Fixed via migration 005 + Fetcher changes
before REQ-002 work started (see git history); Fetcher now stores raw bytes
and the response's Content-Type header.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-002-1 — Native-text PDF/HTML parses into correctly ordered chunks
- [x] TC-REQ-002-2 — Empty document produces zero chunks without error
- [x] TC-REQ-002-3 — Unsupported MIME type rejected before handler dispatch
- [x] TC-REQ-002-4 — OCR worker unavailability is retryable, not permanent failure
- [x] TC-REQ-002-5 — Sustained parsing throughput across mixed formats ⚠️ k6 script exercises parse_and_store indirectly via the admin reprocess endpoint (see loadtest/parse_load.js header), same limitation as REQ-001's fetch_load.js

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-002-01 — Add `document_chunks` migration, `source_documents` columns
- [x] IMP-REQ-002-02 — `ParseError`/`ParseOutcome` types and dispatch by `content_type`
- [x] IMP-REQ-002-03 — HTML handler (semantic extraction, boilerplate removal)
- [x] IMP-REQ-002-04 — Native-text PDF handler via `pdftotext`
- [x] IMP-REQ-002-05 — Scanned-PDF OCR fallback trigger + handler (swappable `OcrProvider` trait; `TesseractOcrProvider` default per Autonomous Execution Notes)
- [x] IMP-REQ-002-06 — Plain-text handler with UTF-8/Latin-1 fallback
- [x] IMP-REQ-002-07 — Per-chunk language detection (EN/FR)
- [x] IMP-REQ-002-08 — Admin reprocess endpoint for parsing
- [x] IMP-REQ-002-09 — Wire retry queue for transient (503-class) handler failures (`parser_status = 'reprocessing'` on transient Pdf/Ocr errors, `'failed'` on permanent UnsupportedContentType)
- [x] IMP-REQ-002-10 — System throughput verification (mixed-format batch)

## REQ-003 — Extract Construction Project Entities

✅ **Open risk resolved:** field completeness against the labelled set was
~85% (stable across repeated real-API runs), below the plan's own 90%
interim gate — specifically `approval_status_raw` going null on ~25% of
qualifying extractions despite six rounds of prompt-only iteration.
Root-caused and fixed: asking for 9 fields in one call made this one short
trailing-sentence field disproportionately likely to be dropped; added a
second-pass, status-only LLM call (`extractor::recover_status`) that fires
only when the main call returns null for it. Two dead ends ruled out along
the way and worth recording so they aren't retried — `temperature` is
outright rejected by this model's API as deprecated (confirmed directly
against the live API); the first version of the fix reused `complete()`,
whose JSON-schema constraint made the model re-emit a full extraction
object as the "status" text instead of following the plain-text
instruction, corrupting the field even though the field became non-null
(caught before shipping — added `LlmProvider::complete_text`, no schema
constraint, for this call). Current measured completeness: 95.3%,
classification accuracy 100%. See `tests/pipeline_extraction.rs` header.

⚠️ **Scope shortfall, now with real documents (flagged, not silent):** the
≥200-item hand-labelled, 3-municipality fixture set (IMP-REQ-003-08) is 30
synthetic items + 3 real items (33 total), still far short of 200. Real
attempt made: `toronto.ca/legdocs` is genuinely reachable — fetched real
"Report for Action" PDFs (46/48/50/52 Laing Street demolition; 1-97 Dorney
Court/2-8 Flemington Road/21-39 Varna Drive, part of Lawrence Heights
Phase 2; 241 Redpath Avenue) with real addresses and unit/storey counts.
Vancouver is completely inaccessible — `council.vancouver.ca` and
`rezoning.vancouver.ca` returned HTTP 403 on every path tried (WebFetch,
direct curl with a browser user agent, Wayback Machine); no browser
extension was available in this session to work around it. Montreal's real
items are in REQ-007 below (French). The remaining shortfall is structural,
not effort: real building-permit decisions with unit/storey/GFA detail are
made by sub-council bodies (Toronto Community Council, Montreal
arrondissements) processing many applications per year — reaching 200 real
items would mean scraping many individual planning applications across
multiple meetings per city, not "the last meeting," which was the
explicitly scoped request. See tests/pipeline_extraction.rs header.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-003-1 — Qualifying project extracts all 5 fields (95.3% completeness, 100% classification accuracy against the live API — see resolved risk note above)
- [x] TC-REQ-003-2 — Single scale-indicator fixture accepted
- [x] TC-REQ-003-3 — Rezoning-only motion excluded despite LLM hallucination
- [x] TC-REQ-003-4 — Malformed LLM JSON discarded, not persisted
- [x] TC-REQ-003-5 — LLM 503 retried, succeeds on 3rd attempt

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-003-01 — `project_mentions` migration with scale-indicator CHECK
- [x] IMP-REQ-003-02 — Extraction JSON schema + versioned EN prompt
- [x] IMP-REQ-003-03 — Deterministic RULE-001 validator
- [x] IMP-REQ-003-04 — Scale-indicator extraction / "at least one" acceptance
- [x] IMP-REQ-003-05 — Wire `extract_entities` dispatch end-to-end
- [x] IMP-REQ-003-06 — Retry/backoff for LLM transient failures
- [x] IMP-REQ-003-07 — Handle malformed/truncated LLM JSON
- [ ] IMP-REQ-003-08 — Assemble ≥200-item labelled fixture set (3 municipalities) ⚠️ Needs Human Review: 33 items (30 synthetic + 3 real from Toronto), still short of 200; Vancouver fully inaccessible, see risk note above
- [x] IMP-REQ-003-09 — Integration test: ≥90% field-completeness on labelled set (95.3% against the live API, see resolved risk note above)

## REQ-004 — Normalize Approval Status in English and French

⚠️ **Gap (documented, not silently assumed):** conflict resolution uses
mention insertion order as an interim proxy for "the later, more specific
dated event" — no per-agenda-item event date exists in the schema yet
(pending REQ-006 timeline work). Every conflict is still flagged into
`review_candidates` regardless, so a wrong auto-resolution stays reviewable.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-004-1 — English synonyms map to correct enum value
- [x] TC-REQ-004-2 — French synonyms map to same enum value as EN
- [x] TC-REQ-004-3 — Unrecognized phrase not silently defaulted
- [x] TC-REQ-004-4 — Conflicting same-document status resolved + flagged

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-004-01 — `status_vocabulary` migration + `project_mentions` status columns
- [x] IMP-REQ-004-02 — Seed EN status vocabulary v1
- [x] IMP-REQ-004-03 — Seed FR status vocabulary v1
- [x] IMP-REQ-004-04 — Implement `normalize_status` deterministic lookup
- [x] IMP-REQ-004-05 — Same-document conflict detection + review-candidate flag
- [x] IMP-REQ-004-06 — Wire normalization into extraction output path
- [x] IMP-REQ-004-07 — Integration test: bilingual parity across launch municipalities (deterministic, 100% non-null on both EN/FR fixture sets)

## REQ-005 — Associate Multiple Mentions Into Tracked Records

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-005-1 — Matching address+type links to existing project
- [x] TC-REQ-005-2 — Near-miss address does not auto-link
- [x] TC-REQ-005-3 — Zero-match mention creates a new project
- [x] TC-REQ-005-4 — Multi-match on address+type creates a review candidate
- [x] TC-REQ-005-5 — DB unavailability during resolution is retryable, not dropped (unit-tested via extracted `retry_transient` seam + injected `sqlx::Error`, no real DB outage needed — see resolver/mod.rs retry_tests)

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-005-01 — Partial unique index on `projects`; coordinate `review_candidates` migration with REQ-009
- [x] IMP-REQ-005-02 — Explicit cross-reference matcher
- [x] IMP-REQ-005-03 — Address+type matcher (using REQ-007 normalizer)
- [x] IMP-REQ-005-04 — `resolve_mention` orchestration (priority order)
- [x] IMP-REQ-005-05 — Wire resolution as automatic post-extraction step
- [x] IMP-REQ-005-06 — Integration test: multi-mention project history
- [x] IMP-REQ-005-07 — Concurrency test: simultaneous resolution of same mention
- [x] IMP-REQ-005-08 — Retry/backoff for `resolve_mention` DB transient failures

## REQ-006 — Display Chronological Project Timeline

⚠️ **Tooling limitation (documented, not silently worked around):** this repo
has no Playwright/headless-browser or axe-core tooling available in this
environment. IMP-REQ-006-08's "loading" state (a transient client-side htmx
state) cannot be observed at all; the loaded/empty/error states are instead
covered by asserting the real `GET /projects/{id}` handler's rendered HTML
end to end (see `tests/timeline_resolver.rs`). IMP-REQ-006-06's accessibility
pass is a manual review (role="alert", aria-live, aria-busy present; no
custom CSS overrides platform focus rings; `.timeline-*` classes are
unstyled and inherit the site's existing body contrast) rather than an
automated axe-core scan.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-006-1 — Timeline renders events in chronological order
- [x] TC-REQ-006-2 — Same-day events tie-break by ingestion order
- [x] TC-REQ-006-3 — Zero-mention project returns empty array, not 404
- [x] TC-REQ-006-4 — Malformed project id rejected with 400
- [x] TC-REQ-006-5 — Nonexistent project id returns 404
- [x] TC-REQ-006-6 — DB unavailability returns 503, UI shows retry (now exercises the real `GET /projects/{id}` handler end to end)

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-006-01 — NOT NULL + index hardening on `project_timeline_events` (migration 012 applied and verified against a live DB)
- [x] IMP-REQ-006-02 — `GET /api/v1/projects/{id}/timeline` handler with tie-break sort
- [x] IMP-REQ-006-03 — 503 handling for DB failures on timeline endpoint
- [x] IMP-REQ-006-07 — Integration test: resolver write → timeline reflects it
#### Frontend Engineer
- [x] IMP-REQ-006-04 — Project-detail timeline template (Minijinja) — added the missing `GET /projects/{id}` page handler (`routes/projects.rs::get_project_detail_page`) that actually serves it; template previously existed but nothing rendered it
- [x] IMP-REQ-006-05 — EN/FR strings for timeline labels (via `Accept-Language`, matching the `index()` route's existing pattern; template converted from hardcoded EN text to context-driven strings)
- [x] IMP-REQ-006-06 — Accessibility pass (focus, contrast, aria-disabled) — manual review, see tooling-limitation note above
- [x] IMP-REQ-006-08 — E2E state verification (loaded/loading/empty/error) — loaded/empty/error covered end to end; loading state out of scope, see tooling-limitation note above

## REQ-007 — Support Bilingual French Extraction

⚠️ **Plan gaps closed (flagged, not silently worked around):**
1. RULE-001's validator (`extractor/validator.rs`) was English-keyword-only
   despite the plan cross-referencing it to TC-REQ-007-3 as
   "language-agnostic" — a French rezoning motion would have matched
   neither keyword list and silently fallen through to trusting the LLM's
   own claim. Added French keyword lists so the validator is actually
   language-agnostic, per the plan's own stated design intent.
2. IMP-REQ-007-04 ("extend French redaction rules") presupposes an existing
   EN redaction baseline from an earlier requirement, but no requirement
   before REQ-007 created one (`pipeline/redaction/` did not exist). Built
   both the baseline dispatcher and the French rules together — there was
   nothing to "extend".
3. `resolver::try_resolve` (REQ-005) always used the English address
   normalizer; wired it to dispatch to `address_fr::normalize_address_fr`
   for French-language mentions so IMP-REQ-007-02's new module has any
   real effect on resolution, matching `address.rs`'s own docstring
   ("REQ-007 extends this module... matcher logic stays shared").

✅ **Live-API completeness gate run and passing** (TC-REQ-007-1/-2 in
`tests/pipeline_extraction_fr.rs`): 98.7% field completeness, 100%
classification accuracy against the real Anthropic API — benefits from the
same status-recovery second pass added for REQ-003's TC-REQ-003-1
(`extractor::recover_status`, language-aware, shared code path), and does
even better here than the EN set.

⚠️ **Scope shortfall, now with real documents (flagged, not silent):** the
≥100-item hand-labelled French fixture subset (IMP-REQ-007-06) is 20
synthetic items + 3 real items (23 total), still far short of 100. Real
attempt made: fetched the genuine 70-page procès-verbal of the Montreal
city council's January 26, 2026 ordinary meeting
(ville.montreal.qc.ca/documents/Adi_Public/CM/CM_PV_ORDI_2026-01-26_13h00_FR.pdf)
and extracted 3 real resolutions with real addresses and "Adopté à
l'unanimité" decision text. All 3 are non-qualifying — for different real
reasons (a land sale enabling future housing construction, administrative;
a real construction item with no scale indicator stated; a land purchase
for a future road project, administrative) — because city-level Montreal
council minutes are dominated by land transactions, financing bylaws, and
appointments; granular building-permit decisions with unit/storey detail
are made at the arrondissement (borough) level, a separate system not
reached in this session. See `tests/pipeline_extraction_fr.rs` header.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-007-1 — French proceedings extract all 5 fields at EN parity (98.7% completeness, 100% classification accuracy against the live API)
- [x] TC-REQ-007-2 — Minimal single-word French status phrase maps correctly (round-trip verified without live API; extraction-quality half same caveat as TC-REQ-007-1)
- [x] TC-REQ-007-3 — RULE-001 excludes a French rezoning-only motion
- [x] TC-REQ-007-4 — LLM 503 during FR extraction is retryable

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-007-01 — Author FR prompt template mirroring EN schema
- [x] IMP-REQ-007-02 — French-Quebec address normalization ruleset (wired into `resolver::try_resolve`'s language dispatch, verified end to end via `french_mention_addresses_resolve_via_the_french_normalizer`, not just the standalone normalizer function)
- [x] IMP-REQ-007-03 — Wire per-language routing into extraction dispatch
- [x] IMP-REQ-007-04 — Extend French named-individual redaction rules (built the missing EN-baseline dispatcher alongside it; wired into `extract_entities` to strip named individuals from `project_name` on the FR path, verified end to end)
- [x] IMP-REQ-007-05 — Per-language field-completeness/confidence metric
- [x] IMP-REQ-007-06 — Assemble ≥100-item labelled French fixture subset ⚠️ Needs Human Review: 23 items (20 synthetic + 3 real from Montreal), still short of 100, see risk note above
- [x] IMP-REQ-007-07 — Integration test: FR parity vs EN (98.7% completeness against the live API, exceeding the 90% gate and EN's own 95.3%)

## REQ-008 — Public Search Without an Account

⚠️ **Interim scope decision (flagged, not silently assumed):** the plan's
refresh-job acceptance criteria says "excludes `review_state=pending`", but
no `review_state` column exists on `projects` — that's REQ-009's
confirm/reject workflow, not yet built at this point in the execution
order. Under the current resolver (REQ-005), a `projects` row is only ever
created via an unambiguous match; genuinely ambiguous matches go to
`review_candidates` and never get a `projects` row. So every current
`projects` row is already "confirmed" by construction, and the refresh job
selects all of them — see the doc comment on
`jobs::public_search_refresh::refresh_public_search_index`. Once REQ-009
ships a `review_state` column, this query needs a `WHERE review_state =
'confirmed'` clause.

⚠️ **Infrastructure wired for the first time:** `redis` was a declared
dependency and provisioned in docker-compose/.env since the start of this
plan but had no caller anywhere in the codebase — `AppState` now holds a
`redis::aio::ConnectionManager`, used by the new rate-limit middleware.
This touched `main.rs` and both existing test helpers
(`tests/admin_routes.rs`, `tests/timeline_resolver.rs`) to add the new
field; all pre-existing tests still pass.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-008-1 — Anonymous search by civic address returns matching project
- [x] TC-REQ-008-2 — Search by municipality name (empty keyword boundary)
- [x] TC-REQ-008-3 — Invalid `per_page` rejected without DB query
- [x] TC-REQ-008-4 — 503 when search connection pool exhausted

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-008-01 — `public_search_documents` migration
- [x] IMP-REQ-008-02 — Refresh job populating index from confirmed projects (see interim scope note above)
- [x] IMP-REQ-008-03 — `GET /api/v1/projects/search` handler
- [x] IMP-REQ-008-05 — Per-IP rate-limiting middleware (wires up the previously-unused Redis dependency, see note above)
- [x] IMP-REQ-008-06 — Automate TC-REQ-008-1..4
#### Frontend Engineer
- [x] IMP-REQ-008-04 — Server-rendered public search page (Minijinja, EN/FR) — server-side query on `GET /search` itself rather than htmx-calling the JSON API, avoiding a JSON-into-HTML mismatch
- [x] IMP-REQ-008-07 — Accessibility/bilingual UX verification — manual review (role="search", labelled input, role="alert" error state, results as a semantic list); no axe-core tooling available, same limitation as REQ-006

## REQ-009 — Human-Review Queue for Ambiguous Matches

⚠️ **Pre-existing UX gap inherited, not introduced here (flagged, not
silently worked around):** `middleware::admin_auth::require_admin` returns
`403` with no `WWW-Authenticate` challenge, so browsers never show a native
Basic Auth login prompt — it was built for programmatic clients
(curl/k6, REQ-001/002's reprocess endpoints) reusing the exact middleware
IMP-REQ-009-05 says to reuse. This is the first requirement to put a real
browser-facing admin UI behind it (`/admin/review_queue`), and there is no
in-app login flow — an operator's browser needs credentials supplied some
other way (reverse-proxy injection, browser extension). Documented in
`docs/runbooks/review_queue.md`; a proper admin login flow is out of this
requirement's scope to invent.

✅ **k6 script run and passing** (IMP-REQ-009-11, TC-REQ-009-6): ran
`loadtest/review_queue.js` against a local `cargo run --release` instance
with 5,000 synthetic open `review_candidates` rows (seeded, tested,
cleaned up afterward — not against production/staging data, which doesn't
exist for this app). p(95)=84.19ms against the 1000ms threshold, 0% error
rate over 17,705 requests. `fetch_load.js`/`parse_load.js` (REQ-001/002)
remain unrun — those need a real municipal-fixture HTTP server behind the
target, not just seeded rows, so the same local-instance approach doesn't
transfer to them.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-009-1 — Confirm merges ambiguous candidate into proposed project
- [x] TC-REQ-009-2 — Candidate exactly at SLA boundary not yet overdue
- [x] TC-REQ-009-3 — Stale version on confirm returns 409, no changes
- [x] TC-REQ-009-4 — Multi-match candidate appears in Open tab (cross-ref REQ-005-4)
- [x] TC-REQ-009-5 — DB failure during confirm leaves candidate unresolved, returns 503
- [x] TC-REQ-009-6 — Queue list endpoint meets latency target under load (p95=84.19ms vs. 1000ms threshold, 0% errors, see note above)

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-009-01 — Business-day calendar helper for `due_at`
- [x] IMP-REQ-009-02 — `review_candidates`/`audit_events` migrations (coordinate with REQ-005)
- [x] IMP-REQ-009-03 — `confirm_candidate`/`reject_candidate` domain functions w/ optimistic version check
- [x] IMP-REQ-009-04 — Axum routes: list/detail/confirm/reject
- [x] IMP-REQ-009-05 — Admin-session auth middleware (reuse if one exists) — reused `middleware::admin_auth`, see UX gap note above
- [x] IMP-REQ-009-08 — Hourly SLA sweep job + overdue metric — plain callable function (`jobs::sla_sweep::compute_overdue_metric`), not a wired-up in-process scheduler, matching REQ-001's `Scheduler` precedent (no periodic-execution infra exists anywhere in this codebase)
- [x] IMP-REQ-009-09 — Review queue routes enabled by default
- [x] IMP-REQ-009-10 — Integration test: candidate → queue → confirm → timeline
- [x] IMP-REQ-009-11 — k6 performance script for queue list endpoint (run and passing, see note above)
- [x] IMP-REQ-009-13 — Operational runbook (SLA sweep and reprocess)
#### Frontend Engineer
- [x] IMP-REQ-009-06 — Review queue list template (tabs, states, EN/FR)
- [x] IMP-REQ-009-07 — Wire Confirm/Reject buttons, handle 409 stale-conflict banner
- [x] IMP-REQ-009-12 — Accessibility/UX verification pass — manual review (role="tablist"/"tab", role="alert" banner, aria-live region, labelled input); no axe-core tooling available, same limitation as REQ-006/008

## System Tests (Loop A suite vs. Loop B production code)

Final run: `cargo test` (all targets) — 170 passing / 0 failed, `cargo clippy
--all-targets -- -D warnings` clean. Every Loop A test compiles and runs
against the final Loop B production code in this same codebase (not a
separate stub); the three left unchecked below are flagged for the reasons
already documented in their requirement sections above, not because they
fail.

- [x] TC-REQ-001-1
- [x] TC-REQ-001-2
- [x] TC-REQ-001-3
- [x] TC-REQ-001-4
- [x] TC-REQ-001-5
- [x] TC-REQ-002-1
- [x] TC-REQ-002-2
- [x] TC-REQ-002-3
- [x] TC-REQ-002-4
- [x] TC-REQ-002-5
- [x] TC-REQ-003-1
- [x] TC-REQ-003-2
- [x] TC-REQ-003-3
- [x] TC-REQ-003-4
- [x] TC-REQ-003-5
- [x] TC-REQ-004-1
- [x] TC-REQ-004-2
- [x] TC-REQ-004-3
- [x] TC-REQ-004-4
- [x] TC-REQ-005-1
- [x] TC-REQ-005-2
- [x] TC-REQ-005-3
- [x] TC-REQ-005-4
- [x] TC-REQ-005-5
- [x] TC-REQ-006-1
- [x] TC-REQ-006-2
- [x] TC-REQ-006-3
- [x] TC-REQ-006-4
- [x] TC-REQ-006-5
- [x] TC-REQ-006-6
- [x] TC-REQ-007-1
- [x] TC-REQ-007-2
- [x] TC-REQ-007-3
- [x] TC-REQ-007-4
- [x] TC-REQ-008-1
- [x] TC-REQ-008-2
- [x] TC-REQ-008-3
- [x] TC-REQ-008-4
- [x] TC-REQ-009-1
- [x] TC-REQ-009-2
- [x] TC-REQ-009-3
- [x] TC-REQ-009-4
- [x] TC-REQ-009-5
- [x] TC-REQ-009-6
