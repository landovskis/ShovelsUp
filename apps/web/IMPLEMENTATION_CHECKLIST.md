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
- [x] IMP-REQ-001-10 — Document `DATA_PIPELINE_INGESTION_ENABLED` flag and rollback

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

⚠️ **Open risk (real, measured — see tests/pipeline_extraction.rs header):**
field completeness against the labelled set is ~85%, stable across repeated
real-API runs, below the plan's own 90% interim gate. The gap is
specifically `approval_status_raw` (a short trailing decision clause is
inconsistently extracted). Six rounds of real prompt iteration (worked
example, effort=high, field reordering) were tried; classification accuracy
(has_mention/physical_work) is 97-100%. Left unresolved rather than
weakening the assertion — matches the plan's own Open Risk on this exact
threshold (Founder, target 2026-07-20).

⚠️ **Scope reduction (flagged, not silent):** the ≥200-item hand-labelled,
3-municipality fixture set (IMP-REQ-003-08) requires real scraped documents
with human ground truth, which cannot be authentically fabricated. Built a
30-item clearly-synthetic set instead — see tests/pipeline_extraction.rs.

### Loop A — Test Plan Implementation Breakdown
- [x] TC-REQ-003-1 — Qualifying project extracts all 5 fields ⚠️ Needs Human Review: real field-completeness is ~85%, not the required ≥90% for all 5 fields — see risk note above
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
- [ ] IMP-REQ-003-08 — Assemble ≥200-item labelled fixture set (3 municipalities) ⚠️ Needs Human Review: scope-reduced to a 30-item synthetic set, see risk note above
- [ ] IMP-REQ-003-09 — Integration test: ≥90% field-completeness on labelled set ⚠️ Test unresolved: real measured completeness is ~85%, below the 90% gate — see risk note above

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

### Loop A — Test Plan Implementation Breakdown
- [ ] TC-REQ-007-1 — French proceedings extract all 5 fields at EN parity
- [ ] TC-REQ-007-2 — Minimal single-word French status phrase maps correctly
- [ ] TC-REQ-007-3 — RULE-001 excludes a French rezoning-only motion
- [ ] TC-REQ-007-4 — LLM 503 during FR extraction is retryable

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-007-01 — Author FR prompt template mirroring EN schema
- [ ] IMP-REQ-007-02 — French-Quebec address normalization ruleset
- [ ] IMP-REQ-007-03 — Wire per-language routing into extraction dispatch
- [ ] IMP-REQ-007-04 — Extend French named-individual redaction rules
- [ ] IMP-REQ-007-05 — Per-language field-completeness/confidence metric
- [ ] IMP-REQ-007-06 — Assemble ≥100-item labelled French fixture subset
- [ ] IMP-REQ-007-07 — Integration test: FR parity vs EN

## REQ-008 — Public Search Without an Account

### Loop A — Test Plan Implementation Breakdown
- [ ] TC-REQ-008-1 — Anonymous search by civic address returns matching project
- [ ] TC-REQ-008-2 — Search by municipality name (empty keyword boundary)
- [ ] TC-REQ-008-3 — Invalid `per_page` rejected without DB query
- [ ] TC-REQ-008-4 — 503 when search connection pool exhausted

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-008-01 — `public_search_documents` migration
- [ ] IMP-REQ-008-02 — Refresh job populating index from confirmed projects
- [ ] IMP-REQ-008-03 — `GET /api/v1/projects/search` handler
- [ ] IMP-REQ-008-05 — Per-IP rate-limiting middleware
- [ ] IMP-REQ-008-06 — Automate TC-REQ-008-1..4
#### Frontend Engineer
- [ ] IMP-REQ-008-04 — Server-rendered public search page (Minijinja, EN/FR)
- [ ] IMP-REQ-008-07 — Accessibility/bilingual UX verification

## REQ-009 — Human-Review Queue for Ambiguous Matches

### Loop A — Test Plan Implementation Breakdown
- [ ] TC-REQ-009-1 — Confirm merges ambiguous candidate into proposed project
- [ ] TC-REQ-009-2 — Candidate exactly at SLA boundary not yet overdue
- [ ] TC-REQ-009-3 — Stale version on confirm returns 409, no changes
- [ ] TC-REQ-009-4 — Multi-match candidate appears in Open tab (cross-ref REQ-005-4)
- [ ] TC-REQ-009-5 — DB failure during confirm leaves candidate unresolved, returns 503
- [ ] TC-REQ-009-6 — Queue list endpoint meets latency target under load

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-009-01 — Business-day calendar helper for `due_at`
- [ ] IMP-REQ-009-02 — `review_candidates`/`audit_events` migrations (coordinate with REQ-005)
- [ ] IMP-REQ-009-03 — `confirm_candidate`/`reject_candidate` domain functions w/ optimistic version check
- [ ] IMP-REQ-009-04 — Axum routes: list/detail/confirm/reject
- [ ] IMP-REQ-009-05 — Admin-session auth middleware (reuse if one exists)
- [ ] IMP-REQ-009-08 — Hourly SLA sweep job + overdue metric
- [ ] IMP-REQ-009-09 — `REVIEW_QUEUE_ENABLED` feature flag
- [ ] IMP-REQ-009-10 — Integration test: candidate → queue → confirm → timeline
- [ ] IMP-REQ-009-11 — k6 performance script for queue list endpoint
- [ ] IMP-REQ-009-13 — Operational runbook (SLA sweep, flag disable, reprocess)
#### Frontend Engineer
- [ ] IMP-REQ-009-06 — Review queue list template (tabs, states, EN/FR)
- [ ] IMP-REQ-009-07 — Wire Confirm/Reject buttons, handle 409 stale-conflict banner
- [ ] IMP-REQ-009-12 — Accessibility/UX verification pass

## System Tests (Loop A suite vs. Loop B production code)
- [ ] TC-REQ-001-1
- [ ] TC-REQ-001-2
- [ ] TC-REQ-001-3
- [ ] TC-REQ-001-4
- [ ] TC-REQ-001-5
- [ ] TC-REQ-002-1
- [ ] TC-REQ-002-2
- [ ] TC-REQ-002-3
- [ ] TC-REQ-002-4
- [ ] TC-REQ-002-5
- [ ] TC-REQ-003-1
- [ ] TC-REQ-003-2
- [ ] TC-REQ-003-3
- [ ] TC-REQ-003-4
- [ ] TC-REQ-003-5
- [ ] TC-REQ-004-1
- [ ] TC-REQ-004-2
- [ ] TC-REQ-004-3
- [ ] TC-REQ-004-4
- [ ] TC-REQ-005-1
- [ ] TC-REQ-005-2
- [ ] TC-REQ-005-3
- [ ] TC-REQ-005-4
- [ ] TC-REQ-005-5
- [x] TC-REQ-006-1
- [x] TC-REQ-006-2
- [x] TC-REQ-006-3
- [x] TC-REQ-006-4
- [x] TC-REQ-006-5
- [x] TC-REQ-006-6
- [ ] TC-REQ-007-1
- [ ] TC-REQ-007-2
- [ ] TC-REQ-007-3
- [ ] TC-REQ-007-4
- [ ] TC-REQ-008-1
- [ ] TC-REQ-008-2
- [ ] TC-REQ-008-3
- [ ] TC-REQ-008-4
- [ ] TC-REQ-009-1
- [ ] TC-REQ-009-2
- [ ] TC-REQ-009-3
- [ ] TC-REQ-009-4
- [ ] TC-REQ-009-5
- [ ] TC-REQ-009-6
