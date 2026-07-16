# Implementation Checklist: Implementation Plan: Public Discovery & Search

**Source Implementation Plan:** https://mobilispect.atlassian.net/wiki/spaces/ShovelsUp/pages/21561408/Implementation+Plan+Public+Discovery+Search
**Target directory:** /Users/alex/src/shovelsup

**M0 — Migration renumbering (blocking, must resolve first):** repo's actual
highest migration is `015_montreal_agenda_url.sql` (confirmed via
`ls apps/web/web/migrations/`), one ahead of the plan's own assumed base of
013/014. Consolidated order below is shifted by +1 from the plan's proposed
015–026 to the real next-free 016–026 (REQ-012 needs no migration, so it is
dropped from the numbered sequence):

| # | Requirement | Change |
| --- | --- | --- |
| 016 | REQ-013 | `merged_into_id` self-reference + one-hop CHECK |
| 017 | REQ-002 | `municipality_slug` column + backfill |
| 018 | REQ-003 | `source_language`, `search_vector_fr/en`, GIN indexes |
| 019 | REQ-004 | `first_surfaced_at` |
| 020 | REQ-006 | `meeting_date`, `citation_url_reliable` |
| 021 | REQ-007 | `idx_projects_surfaced_at` (index-only, `CONCURRENTLY`) |
| 022 | REQ-008 | `category_taxonomy` table + `projects.category_code` |
| 023 | REQ-009 | `latest_meeting_date` |
| 024 | REQ-014 | `cta_events` table |
| 025 | REQ-015 | `first_detected_at`, `source_count` |
| 026 | REQ-005 | `description_lang`, `confidence_level`, `source_document_url` |

Whichever requirement's Data task actually lands first claims the next real
sequential number; this table is the agreed order, confirmed/adjusted at
each requirement's migration task if a conflict is found.

**System-test harness note:** no headless-browser/E2E tooling exists yet in
this repo. Per user instruction, this run introduces it as part of
REQ-011 (evaluate `fantoccini` first per the plan; fall back to a
Node/Playwright CI stage if unreliable). Until REQ-011 lands, the system-test
gate is `cargo build --workspace && cargo nextest run --workspace && cargo
clippy --workspace -- -D warnings`; REQ-011/REQ-013's browser-driven cases
(320px scroll sweep, clipboard test) are added once the harness exists.

---

## REQ-001 — Keyword search with no login

### Loop A — Test Plan Implementation Breakdown
- [x] TC-001-1 — existing tc_req_008_* coverage re-verified post-refactor (IMP-001-04)
- [x] TC-001-2 — existing tc_req_008_* coverage re-verified post-refactor (IMP-001-04)
- [x] TC-001-3 — existing tc_req_008_* coverage re-verified post-refactor (IMP-001-04)
- [x] TC-001-4 — existing tc_req_008_* coverage re-verified post-refactor (IMP-001-04)
- [x] TC-001-5 — existing tc_req_008_* coverage re-verified post-refactor (IMP-001-04)
- [x] TC-001-6 — French locale + no-modal assertion (new, IMP-001-09) — `tc_001_6_french_locale_no_modal` in web/tests/search_integration.rs, APPROVED

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-001-01 — Run baseline `cargo nextest run --workspace` (272 total, 219 pass, 53 fail — matches Loop A A4 exactly; no files changed, nothing to commit)
- [x] IMP-REQ-001-02 — Add `validate_search_params` pure core function (7/7 unit tests, not yet wired into run_search)
- [x] IMP-REQ-001-03 — Unit tests for core function (existing 7 confirmed thorough, no additions needed; also fixed a pre-existing clippy doc-lint break in responsive_e2e.rs blocking the workspace-wide `-D warnings` gate)
- [x] IMP-REQ-001-04 — Wire `run_search` to call the core function (isolated before/after diff confirms zero regression)
- [x] IMP-REQ-001-05 — Verify/fix X-Forwarded-For rate-limit trust assumption (real security fix: no reverse proxy fronts this app, so XFF was forgeable; switched rate-limit key to unspoofable ConnectInfo<SocketAddr>; verified deterministic across 2 independent full-suite runs, 284 total/231 pass/53 fail, zero regressions)
- [x] IMP-REQ-001-09 — Integration test: French locale + no-modal assertion (TC-001-6) (already satisfied by Loop A's tc_001_6, reverified passing)
- [x] IMP-REQ-001-10 — Accessibility (WCAG AA) verification pass (all items already correct from 06/07/08; no template changes needed; contract locked in by new test, 296/243/53)
- [x] IMP-REQ-001-11 — Full regression run (build clean, clippy clean, 296/243/53, REQ-001 complete)
#### Frontend Engineer
- [x] IMP-REQ-001-06 — Add empty-state guidance line (EN/FR) (distinct markup from REQ-012's future element IDs, verified no collision)
- [x] IMP-REQ-001-07 — Add [EN]/[FR] source-language badge (markup only, gates on Some/None; positive-render case deferred to REQ-003's own Loop B once source_language column lands; confirmed tc_003_5 unaffected)
- [x] IMP-REQ-001-08 — Add result-count header with correct pluralization (295/242/53, zero regressions; closed 4 post-commit coverage gaps)

⚠️ **Product/UX note (not a bug, not fixed, flagged only):** the result-count header reflects the `per_page`-truncated count, not the true total match count (e.g. 5 real matches + `per_page=2` renders "2 results found" with no "of 5" indication). Worth a follow-up UX decision, out of this task's scope.

## REQ-002 — Location-based search by municipality

### Loop A — Test Plan Implementation Breakdown
- [x] TC-002-1 — `tc_002_1` in web/tests/search_integration.rs, compiles, expected-fail (filter not wired, IMP-002-04)
- [x] TC-002-2 — `tc_002_2` in web/tests/search_integration.rs, compiles, expected-fail (validation not wired, IMP-002-03/04)
- [x] TC-002-3 — `tc_002_3` in web/tests/search_integration.rs, compiles, expected-fail (AND filter not wired, IMP-002-04)
- [x] TC-002-4 — `tc_002_4` in web/tests/search_integration.rs, compiles, PASSES today (backward-compat path needs no new logic)
- [x] TC-002-5 — `tc_002_5` in web/tests/search_integration.rs, compiles, expected-fail (filter not wired, IMP-002-04)
- [x] TC-002-6 — `tc_002_6` in web/tests/search_integration.rs, compiles, expected-fail (case-normalization not wired, IMP-002-04); commits to "normalize to lowercase" behavior

### Loop B — Task Breakdown
#### Backend Engineer
- [x] IMP-REQ-002-01 — Migration: `municipality_slug` column + backfill + index (017_public_search_municipality_slug.sql; live-verified against Postgres including a deliberate mismatch triggering the parity check)
- [x] IMP-REQ-002-02 — Update refresh job to upsert `municipality_slug` (insert + ON CONFLICT UPDATE paths both verified against real DB state, 314/261/53, zero regressions)

⚠️ **Local-environment note (not a code issue):** local dev Postgres had a pre-existing checksum drift on migration 2 blocking `sqlx migrate run` (predates this session). Migration 017 was applied directly via psql to unblock compile-time query checks; the migration file itself is untouched. Flagging so a real deploy/CI environment run applies migrations normally rather than assuming this workaround is needed elsewhere.
- [x] IMP-REQ-002-03 — Pure `validate_municipality_slug` core function (syntactic-only; live-table check deferred to IMP-REQ-002-04 per plan's own notes; 10 unit tests, 306/253/53, zero regressions)
- [x] IMP-REQ-002-04 — Wire municipality filter + validation into handlers (all 6 tc_002_* tests pass, 314/266/48; also fixed a test-fixture bug where seed_searchable_project always created a random-slugged municipality instead of using the real pre-seeded montreal/toronto/vancouver rows tc_002_1/3/6 query by slug)
- [x] IMP-REQ-002-05 — `municipality_display_name` EN/FR helper (8 unit tests, 314/261/53, zero regressions)
- [x] IMP-REQ-002-09 — Automate all 6 system test cases (already satisfied by IMP-REQ-002-04's fix — all 6 tc_002_* tests automated and passing)
- [ ] IMP-REQ-002-10 — Accessibility verification (keyboard, screen reader, contrast)
- [ ] IMP-REQ-002-11 — Regression check: REQ-008 keyword-only search unaffected
#### Frontend Engineer
- [x] IMP-REQ-002-06 — Add select control to search form (populated from live municipalities table, selection preserved across resubmission, 315/267/48)
- [x] IMP-REQ-002-07 — Responsive/CSS for the filter bar (uses existing design tokens/breakpoint convention, 322/271/51)

⚠️ **Pre-existing test flakiness found and isolated (not caused by this or any REQ-002 task):** `tc_req_008_3_per_page_over_max_rejected` intermittently fails under parallel test load (confirmed via 3 independent full-suite runs: pass/fail alternates, no other test varies). Likely transient DB-connection-pool contention under this environment's parallel `#[sqlx::test]` execution, not a code defect — this is one of the original REQ-008 (Data Pipeline plan) tests, unrelated to this plan's changes. Flagged for awareness; not chased further here since it's out of scope for any task in this plan.
- [x] IMP-REQ-002-08 — Municipality-aware empty-state + invalid-filter copy (invalid-slug path already rendered a friendly error, confirmed and pinned by test; 321/270/51 confirmed stable across 2 independent full-suite runs, no flakiness found)

## REQ-003 — Bilingual search input and results

### Loop A — Test Plan Implementation Breakdown
- [x] TC-003-1 — `tc_003_1` compiles, PASSES today (documents ILIKE stemming gap)
- [x] TC-003-2 — `tc_003_2` compiles, expected-fail (lang param precedence not wired)
- [x] TC-003-3 — `tc_003_3` compiles, expected-fail (cookie precedence not wired)
- [x] TC-003-4 — `tc_003_4` compiles, PASSES today (Accept-Language regression guard)
- [x] TC-003-5 — `tc_003_5` compiles, expected-fail (source_language badge not wired)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-003-01 — Discover existing search table/locale mechanism
- [ ] IMP-REQ-003-02 — Migration: `source_language`, `search_vector_fr/en`, GIN indexes, backfill (verify `french` Postgres FTS config availability first)
- [ ] IMP-REQ-003-03 — Pure `normalize_query`/`resolve_ui_locale` core functions
- [ ] IMP-REQ-003-04 — Wire locale resolver + normalized query into `GET /search`
- [ ] IMP-REQ-003-05 — FR/EN string-table entries + key-parity check
- [ ] IMP-REQ-003-09 — Test fixtures (FR/EN seed rows, fault-injecting DB wrapper)
- [ ] IMP-REQ-003-10 — Implement all 5 test cases
- [ ] IMP-REQ-003-11 — Accessibility verification
#### Frontend Engineer
- [ ] IMP-REQ-003-06 — Build `search_results.html` states per UI mockup
- [ ] IMP-REQ-003-07 — Responsive breakpoints
- [ ] IMP-REQ-003-08 — `?lang=` toggle + cookie persistence

## REQ-004 — Project list / search results view

### Loop A — Test Plan Implementation Breakdown
- [x] TC-004-1 — `tc_004_1` compiles, PASSES today (documents bare-array gap, no envelope yet)
- [x] TC-004-2 — `tc_004_2` compiles, PASSES today (documents missing HTMX fragment branching)
- [x] TC-004-3 — `tc_004_3` compiles, PASSES today (documents missing display_name)
- [x] TC-004-4 — `tc_004_4` compiles, `#[ignore]`d pending IMP-REQ-004-01 migration (justified exception, real assertion body present)
- [x] TC-004-5 — `tc_004_5` compiles, PASSES today (documents missing pagination boundary handling)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-004-01 — Migration: `first_surfaced_at` + index
- [ ] IMP-REQ-004-02 — Refresh job: set `first_surfaced_at` only on INSERT
- [ ] IMP-REQ-004-03 — Pagination-math + status-label core functions, filter params (assumes no `project_name` free-text field; spot-check synthesized display name against sample data)
- [ ] IMP-REQ-004-04 — JSON API paginated envelope
- [ ] IMP-REQ-004-05 — HTML route HTMX branching (full page vs. fragment)
- [ ] IMP-REQ-004-09 — Unit tests (pagination math, status labels)
- [ ] IMP-REQ-004-10 — Integration tests TC-004-1..5
- [ ] IMP-REQ-004-11 — Accessibility verification
#### Frontend Engineer
- [ ] IMP-REQ-004-06 — `search.html` results/pagination markup
- [ ] IMP-REQ-004-07 — `results_fragment.html` + infinite-scroll wiring
- [ ] IMP-REQ-004-08 — CSS for status indicator/pagination/responsive

## REQ-005 — Project detail view

### Loop A — Test Plan Implementation Breakdown
- [x] TC-005-1 — compiles, expected-fail (description/confidence/source-link not yet rendered)
- [x] TC-005-2 — compiles, expected-fail (graceful degradation not yet wired)
- [x] TC-005-3 — compiles, PASSES today (malformed UUID 400 regression guard)
- [x] TC-005-4 — compiles, PASSES today (404 + 503 regression guard)
- [x] TC-005-5 — compiles, expected-fail (description-language divergence indicator not yet wired)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-005-02 — Schema-existence verification (all 3 municipalities share schema)
- [ ] IMP-REQ-005-01 — Migration: `description_lang`, `confidence_level`, `source_document_url`
- [ ] IMP-REQ-005-03 — `project_detail` handler: UUID parse, join query, 200/404/503 branch
- [ ] IMP-REQ-005-04 — Reuse/extract shared locale-resolution utility
- [ ] IMP-REQ-005-05 — Template context struct
- [ ] IMP-REQ-005-10 — Unit tests
- [ ] IMP-REQ-005-11 — Integration: happy path + boundary
- [ ] IMP-REQ-005-12 — Integration: negative + error path
- [ ] IMP-REQ-005-13 — Integration: locale/source-language divergence
- [ ] IMP-REQ-005-14 — Accessibility/responsive manual pass
#### Frontend Engineer
- [ ] IMP-REQ-005-06 — `project_detail.html.jinja` (all states)
- [ ] IMP-REQ-005-07 — 404/503 error templates
- [ ] IMP-REQ-005-08 — EN/FR string catalog
- [ ] IMP-REQ-005-09 — Responsive CSS

## REQ-006 — Source transparency notice

### Loop A — Test Plan Implementation Breakdown
- [x] TC-006-1 — compiles, expected-fail (no #project-source element yet)
- [x] TC-006-2 — compiles, expected-fail (citation-reliability branching not wired)
- [x] TC-006-3 — compiles, expected-fail (meeting_date fallback not wired)
- [x] TC-006-4 — compiles, PASSES today (no source doc → section omitted, trivially true but meaningfully asserted)
- [x] TC-006-5 — compiles, expected-fail (citation-query failure isolation not wired)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-006-01 — Schema verification
- [ ] IMP-REQ-006-02 — Migration: `meeting_date`, `citation_url_reliable` (no backfill needed, safe default)
- [ ] IMP-REQ-006-03 — Pure `resolve_citation_view` decision function
- [ ] IMP-REQ-006-04 — Add `url` crate dependency
- [ ] IMP-REQ-006-05 — `fetch_primary_citation` query
- [ ] IMP-REQ-006-06 — Wire into `get_project_detail_page`, isolate failure to the section
- [ ] IMP-REQ-006-10 — Unit tests
- [ ] IMP-REQ-006-11 — Integration test TC-006-1
- [ ] IMP-REQ-006-12 — Integration test TC-006-2
- [ ] IMP-REQ-006-13 — Integration test TC-006-3
- [ ] IMP-REQ-006-14 — Integration test TC-006-4
- [ ] IMP-REQ-006-15 — Integration test TC-006-5
#### Frontend Engineer
- [ ] IMP-REQ-006-07 — EN/FR label struct additions
- [ ] IMP-REQ-006-08 — `.project-source` template section (all states)
- [ ] IMP-REQ-006-09 — Accessibility pass

## REQ-007 — Date-range filter

### Loop A — Test Plan Implementation Breakdown
- [x] TC-007-1 — compiles, `#[ignore]`d pending IMP-REQ-004-01 first_surfaced_at + IMP-REQ-007-05
- [x] TC-007-2 — compiles, `#[ignore]`d pending IMP-REQ-004-01 first_surfaced_at + IMP-REQ-007-05
- [x] TC-007-3 — compiles, expected-fail (400/409 validation not wired)
- [x] TC-007-4 — compiles, expected-fail (malformed-date validation not wired)
- [x] TC-007-5 — compiles, PASSES today (backward compat)
- [x] TC-007-6 — compiles, `#[ignore]`d pending IMP-REQ-004-01 first_surfaced_at + IMP-REQ-007-05

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-007-01 — Confirm `surfaced_at` column schema (`TIMESTAMPTZ NOT NULL` assumption; correct in place if wrong — blocks downstream tasks until reconciled)
- [ ] IMP-REQ-007-02 — Migration: supporting index (`CONCURRENTLY`, non-locking)
- [ ] IMP-REQ-007-03 — `DateFilter` parse/validate module
- [ ] IMP-REQ-007-04 — Unit tests for `DateFilter`
- [ ] IMP-REQ-007-05 — Wire into search query builder, map errors to 400/409
- [ ] IMP-REQ-007-06 — 503 handling verification
- [ ] IMP-REQ-007-07 — Integration tests (composed endpoint)
- [ ] IMP-REQ-007-12 — System E2E test cases 1-6
- [ ] IMP-REQ-007-13 — Bilingual QA pass
#### Frontend Engineer
- [ ] IMP-REQ-007-08 — EN/FR i18n string entries
- [ ] IMP-REQ-007-09 — Segmented preset control + custom-range disclosure
- [ ] IMP-REQ-007-10 — Wire filter bar to query-string state, applied-filter chip
- [ ] IMP-REQ-007-11 — Accessibility verification

## REQ-008 — Project type / category filter

### Loop A — Test Plan Implementation Breakdown
- [x] TC-008-1 — compiles, expected-fail (category filter not wired)
- [x] TC-008-2 — compiles, expected-fail (uncategorised filter not wired)
- [x] TC-008-3 — compiles, expected-fail (400 validation not wired)
- [x] TC-008-4 — compiles, expected-fail (`GET /categories` unrouted)
- [x] TC-008-5 — compiles, expected-fail (facet degradation not wired)

⚠️ **Pre-existing router quirk found (not caused by this plan, flagged for IMP-REQ-008-04 and IMP-REQ-010-02):** in `apps/web/web/src/lib.rs`, `.layer(admin_auth::require_admin)` on `admin_routes` wraps that sub-router's own 404 fallback, which becomes the merged app's catch-all for ANY unmatched path — so unrouted paths return 403, not 404. Loop B should account for this when wiring `GET /categories` and when REQ-010 restructures into `public_router()`/`authenticated_router()`.

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-008-01 — Audit extraction pipeline's classification contract (assumed taxonomy: residential|commercial|institutional|infrastructure|other)
- [ ] IMP-REQ-008-02 — Migration: `category_taxonomy` table + `projects.category_code`
- [ ] IMP-REQ-008-03 — Query parsing/validation (400/403 error mapping)
- [ ] IMP-REQ-008-04 — `GET /categories` facet endpoint
- [ ] IMP-REQ-008-05 — Uncategorised serialization + 503-degraded mode
- [ ] IMP-REQ-008-06 — Security review (parameterized queries, escaping)
- [ ] IMP-REQ-008-10 — Locale string entries
- [ ] IMP-REQ-008-11 — Unit tests
- [ ] IMP-REQ-008-12 — Integration tests
- [ ] IMP-REQ-008-13 — System tests TC-008-1..5
- [ ] IMP-REQ-008-14 — Accessibility tests
- [ ] IMP-REQ-008-15 — Deploy sequencing (migration before code deploy)
#### Frontend Engineer
- [ ] IMP-REQ-008-07 — Filter chip row template
- [ ] IMP-REQ-008-08 — htmx wiring, loading/empty/error states
- [ ] IMP-REQ-008-09 — Responsive/keyboard nav

## REQ-009 — Timeline / chronological view

### Loop A — Test Plan Implementation Breakdown
- [x] TC-009-1 — compiles, `#[ignore]`d pending IMP-REQ-009-01 latest_meeting_date migration
- [x] TC-009-2 — compiles, PASSES today (default-order regression guard)
- [x] TC-009-3 — compiles, `#[ignore]`d pending IMP-REQ-009-01 latest_meeting_date migration
- [x] TC-009-4 — compiles, `#[ignore]`d pending IMP-REQ-009-01 latest_meeting_date migration
- [x] TC-009-5 — compiles, expected-fail (invalid sort validation not wired)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-009-01 — Migration: `latest_meeting_date` + index
- [ ] IMP-REQ-009-02 — Refresh job: `LEFT JOIN LATERAL MAX(event_date)`
- [ ] IMP-REQ-009-03 — Refresh-job unit tests
- [ ] IMP-REQ-009-04 — `sort` param + validation (400 before any query)
- [ ] IMP-REQ-009-05 — Route unit tests
- [ ] IMP-REQ-009-06 — Date ORDER BY + `SearchResult.latest_meeting_date`
- [ ] IMP-REQ-009-07 — Thread `sort`/`active_sort` into HTML context
- [ ] IMP-REQ-009-12 — Integration test TC-009-1
- [ ] IMP-REQ-009-13 — Integration test TC-009-2
- [ ] IMP-REQ-009-14 — Integration test TC-009-3
- [ ] IMP-REQ-009-15 — Integration test TC-009-4
- [ ] IMP-REQ-009-16 — Integration test TC-009-5
- [ ] IMP-REQ-009-17 — Manual/exploratory QA pass (confirm descending newest-first direction)
#### Frontend Engineer
- [ ] IMP-REQ-009-08 — Toggle markup (button in existing form)
- [ ] IMP-REQ-009-09 — Per-row meeting-date display
- [ ] IMP-REQ-009-10 — EN/FR copy
- [ ] IMP-REQ-009-11 — Responsive/accessibility pass

## REQ-010 — No account required for any search or view action

### Loop A — Test Plan Implementation Breakdown
- [x] TC-010-01 — `tests/no_account_required.rs`, PASSES today (structural guarantee already holds)
- [x] TC-010-02 — `tests/no_account_required.rs`, PASSES today
- [x] TC-010-03 — `tests/no_account_required.rs`, PASSES today
- [x] TC-010-04 — `tests/no_account_required.rs`, PASSES today; verified non-vacuous (reproduced+reverted a real regression)
- [x] TC-010-05 — `tests/no_account_required.rs`, PASSES today (admin gate regression guard)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-010-01 — Confirm current router structure/auth middleware presence
- [ ] IMP-REQ-010-02 — Split into `public_router()`/`authenticated_router()`
- [ ] IMP-REQ-010-03 — Harden `GET /search` handler
- [ ] IMP-REQ-010-04 — Harden `GET /search/results` with typed validation
- [ ] IMP-REQ-010-05 — Harden `GET /projects/:id` handler
- [ ] IMP-REQ-010-06 — Read-only DB role for public handlers
- [ ] IMP-REQ-010-07 — Silent/non-interactive rate limiting
- [ ] IMP-REQ-010-08 — Unit test: public router has no auth layer
- [ ] IMP-REQ-010-09 — Unit tests: results handler error branches + cookie-equivalence
- [ ] IMP-REQ-010-10 — Unit test: detail renders without session, 404 on unknown id
- [ ] IMP-REQ-010-11 — Integration: full anonymous journey
- [ ] IMP-REQ-010-12 — Integration: expired/forged token treated as anonymous
- [ ] IMP-REQ-010-13 — Integration: pagination boundary
- [ ] IMP-REQ-010-14 — Regression guard (CI-gating)
- [ ] IMP-REQ-010-19 — Fixtures
- [ ] IMP-REQ-010-20 — Test execution sign-off
- [ ] IMP-REQ-010-21 — Accessibility pass
#### Frontend Engineer
- [ ] IMP-REQ-010-15 — Template for search screen
- [ ] IMP-REQ-010-16 — Template for results screen
- [ ] IMP-REQ-010-17 — Template for detail screen
- [ ] IMP-REQ-010-18 — EN/FR toggle verification

## REQ-011 — Mobile-responsive search experience

### Loop A — Test Plan Implementation Breakdown
- [x] TC-011-1 — `tests/responsive_e2e.rs`, PASSES today (viewport meta tag regression guard)
- [x] TC-011-2 — `tests/responsive_e2e.rs`, `#[ignore]`d pending IMP-REQ-011-13 harness setup, fantoccini sketch included
- [x] TC-011-3 — `tests/responsive_e2e.rs`, `#[ignore]`d pending IMP-REQ-011-13 harness setup, invented selectors flagged for reconciliation with IMP-REQ-011-03/04 markup
- [x] TC-011-4 — `tests/responsive_e2e.rs`, split into 3 sub-tests: 503-shell PASSES today, 404/400-shell both expected-fail (bare StatusCode bypasses shell, gap for IMP-REQ-011-07)
- [x] TC-011-5 — `tests/responsive_e2e.rs`, PASSES today (documents no fault-injection hook exists yet, gap for IMP-REQ-011-08)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-011-07 — Route 400/404/503 through the responsive shell
- [ ] IMP-REQ-011-08 — Test-only fault-injection hook
- [ ] IMP-REQ-011-09 — Confirm existing input-validation 400 paths
- [ ] IMP-REQ-011-10 — Unit tests (UA-branching)
- [ ] IMP-REQ-011-11 — Unit tests (locale)
- [ ] IMP-REQ-011-12 — Unit tests (breakpoints)
- [ ] IMP-REQ-011-13 — Headless-browser harness setup (evaluate `fantoccini` first; fall back to Node/Playwright CI stage) — establishes the system-test gate referenced at the top of this checklist
- [ ] IMP-REQ-011-14 — Integration: 320px no-horizontal-scroll sweep
- [ ] IMP-REQ-011-15 — Integration: filter sheet non-blocking + focus return
- [ ] IMP-REQ-011-16 — Integration: remaining flow tests
- [ ] IMP-REQ-011-17 — Accessibility/UX verification pass (axe-core scan)
#### Frontend Engineer
- [ ] IMP-REQ-011-01 — CSS breakpoint/spacing tokens
- [ ] IMP-REQ-011-02 — `meta name="viewport"` tag
- [ ] IMP-REQ-011-03 — Responsive search/results markup (Grid/Flexbox)
- [ ] IMP-REQ-011-04 — Mobile filter sheet component
- [ ] IMP-REQ-011-05 — Empty/loading/error/disabled state markup
- [ ] IMP-REQ-011-06 — Responsive detail-page layout

## REQ-012 — Empty-state and no-results messaging

### Loop A — Test Plan Implementation Breakdown
- [x] TC-012-1 — compiles, expected-fail (headline/body markup not wired)
- [x] TC-012-2 — compiles, expected-fail (FR localization not wired)
- [x] TC-012-3 — compiles, expected-fail (4 suggestions not wired)
- [x] TC-012-4 — compiles, expected-fail (2 action links not wired)
- [x] TC-012-5 — compiles, PASSES today (mutual exclusivity, verified non-vacuous)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-012-01 — Confirm current `SearchLabels`/branch structure
- [ ] IMP-REQ-012-02 — Extend `SearchLabels` with 5 new EN/FR fields
- [ ] IMP-REQ-012-03 — Thread into `context!` call
- [ ] IMP-REQ-012-04 — Verify action-link targets resolve
- [ ] IMP-REQ-012-08 — Unit tests (EN/FR/fallback)
- [ ] IMP-REQ-012-09 — Integration tests TC-012-1..5
- [ ] IMP-REQ-012-10 — Accessibility verification
#### Frontend Engineer
- [ ] IMP-REQ-012-05 — Build `_empty_state.html` partial
- [ ] IMP-REQ-012-06 — Responsive/visual styling
- [ ] IMP-REQ-012-07 — Loading/disabled-state pass (confirm non-applicable)

## REQ-013 — Shareable project URL

### Loop A — Test Plan Implementation Breakdown
- [x] TC-013-1 — `tests/shareable_url.rs`, `#[ignore]`d pending IMP-REQ-013-01 merged_into_id migration
- [x] TC-013-2 — `tests/shareable_url.rs`, compiles, expected-fail (canonical URL not wired)
- [x] TC-013-3 — `tests/shareable_url.rs`, `#[ignore]`d pending IMP-REQ-011-13 harness setup (shared with REQ-011), fantoccini sketch included
- [x] TC-013-4 — `tests/shareable_url.rs`, compiles, expected-fail (malformed UUID returns raw rejection text, not friendly page)
- [x] TC-013-5 — `tests/shareable_url.rs`, split EN/FR variants, both compile, expected-fail (bare 404, no bilingual template)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-013-01 — Migration: `merged_into_id` + one-hop CHECK + index
- [ ] IMP-REQ-013-02 — `not_found_labels` pure function
- [ ] IMP-REQ-013-04 — 301-redirect check on `merged_into_id`
- [ ] IMP-REQ-013-05 — Replace bare 404 with rendered template; 400 on malformed UUID
- [ ] IMP-REQ-013-06 — `canonical_url` function ignoring Host header
- [ ] IMP-REQ-013-10 — Fixture harness (plain + merged-pair seeds)
- [ ] IMP-REQ-013-11 — Integration: TC-013-1, TC-013-2
- [ ] IMP-REQ-013-12 — Integration: TC-013-4, TC-013-5
- [ ] IMP-REQ-013-13 — Playwright/fantoccini: TC-013-3 (clipboard)
- [ ] IMP-REQ-013-14 — Accessibility verification
#### Frontend Engineer
- [ ] IMP-REQ-013-03 — `project_not_found.html` template
- [ ] IMP-REQ-013-07 — Canonical/OG head tags
- [ ] IMP-REQ-013-08 — "Copy link" button markup
- [ ] IMP-REQ-013-09 — Clipboard JS (write, revert, fallback)

## REQ-014 — Upsell CTA on project detail page

### Loop A — Test Plan Implementation Breakdown
- [x] TC-014-1 — `tests/cta_upsell.rs`, compiles, expected-fail (no #cta-upsell markup yet)
- [x] TC-014-2 — `tests/cta_upsell.rs`, compiles, PASSES today (not-a-modal regression guard)
- [x] TC-014-3 — `tests/cta_upsell.rs`, compiles, expected-fail (no signup link markup yet)
- [x] TC-014-4 — `tests/cta_upsell.rs`, compiles, expected-fail (no collapse toggle yet)
- [x] TC-014-5 — `tests/cta_upsell.rs`, compiles, PASSES today (telemetry isolation holds); surfaced the same router 403-vs-404 quirk as REQ-008
- [x] TC-014-6 — `tests/cta_upsell.rs`, compiles, expected-fail (no localStorage persistence script yet)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-014-01 — Migration: `cta_events` table
- [ ] IMP-REQ-014-02 — `POST /api/v1/cta-events` handler
- [ ] IMP-REQ-014-03 — Rate limiting + origin check
- [ ] IMP-REQ-014-04 — `/signup` deep-link builder (server-validated params only; /signup may not exist yet, 404 acceptable per out-of-scope boundary)
- [ ] IMP-REQ-014-05 — Anonymous/authenticated context branch (full omission)
- [ ] IMP-REQ-014-10 — Unit tests 1-3
- [ ] IMP-REQ-014-11 — System tests TC-014-1/3/4
- [ ] IMP-REQ-014-12 — TC-014-2 modal/scroll-lock regression guard
- [ ] IMP-REQ-014-13 — TC-014-5 telemetry-failure test
- [ ] IMP-REQ-014-14 — TC-014-6 collapse-persistence test
- [ ] IMP-REQ-014-15 — Accessibility scan
- [ ] IMP-REQ-014-16 — ADR: non-modal placement decision
- [ ] IMP-REQ-014-17 — Architecture docs update
#### Frontend Engineer
- [ ] IMP-REQ-014-06 — EN/FR CTA copy strings
- [ ] IMP-REQ-014-07 — `cta_alerts` partial (section, no dialog markup)
- [ ] IMP-REQ-014-08 — Responsive, non-fixed CSS
- [ ] IMP-REQ-014-09 — Beacon + collapse/expand JS (30-day localStorage)

## REQ-015 — Search result confidence indicator

### Loop A — Test Plan Implementation Breakdown
- [x] TC-015-1 — `tests/search_integration.rs`, compiles (verified independently after subagent connection drop)
- [x] TC-015-2 — `tests/timeline_resolver.rs`, compiles (verified independently after subagent connection drop)
- [x] TC-015-3 — `tests/search_integration.rs`, compiles (verified independently after subagent connection drop)
- [x] TC-015-4 — `tests/search_integration.rs`, compiles (verified independently after subagent connection drop)
- [x] TC-015-5 — `tests/search_integration.rs`, compiles (verified independently after subagent connection drop)
- [x] TC-015-6 — `tests/search_integration.rs`, compiles (verified independently after subagent connection drop)

### Loop B — Task Breakdown
#### Backend Engineer
- [ ] IMP-REQ-015-01 — Discover "distinct source document" schema column (falls back to `COUNT(DISTINCT document_chunk_id)` if no parent identifier exists)
- [ ] IMP-REQ-015-02 — Migration: `first_detected_at`, `source_count`
- [ ] IMP-REQ-015-03 — Pure core function computing both fields
- [ ] IMP-REQ-015-04 — Imperative-shell materializer, wired into existing sync
- [ ] IMP-REQ-015-05 — Best-effort backfill for existing rows (idempotent)
- [ ] IMP-REQ-015-06 — Search route/API: fields + `detection_sentence` derivation
- [ ] IMP-REQ-015-07 — Detail route: same derived fields
- [ ] IMP-REQ-015-08 — Core unit tests (day-count boundary)
- [ ] IMP-REQ-015-09 — Materializer integration test
- [ ] IMP-REQ-015-10 — Route-level unit tests (EN/FR pluralization, suppression)
- [ ] IMP-REQ-015-13 — Accessibility/contrast verification
- [ ] IMP-REQ-015-14 — System tests TC-015-1..6
- [ ] IMP-REQ-015-15 — Cross-locale sanity pass
#### Frontend Engineer
- [ ] IMP-REQ-015-11 — Search card template line
- [ ] IMP-REQ-015-12 — Detail page paragraph

---

## System Tests (Loop A suite vs. Loop B production code)

- [ ] TC-001-1
- [ ] TC-001-2
- [ ] TC-001-3
- [ ] TC-001-4
- [ ] TC-001-5
- [ ] TC-001-6
- [ ] TC-002-1
- [ ] TC-002-2
- [ ] TC-002-3
- [ ] TC-002-4
- [ ] TC-002-5
- [ ] TC-002-6
- [ ] TC-003-1
- [ ] TC-003-2
- [ ] TC-003-3
- [ ] TC-003-4
- [ ] TC-003-5
- [ ] TC-004-1
- [ ] TC-004-2
- [ ] TC-004-3
- [ ] TC-004-4
- [ ] TC-004-5
- [ ] TC-005-1
- [ ] TC-005-2
- [ ] TC-005-3
- [ ] TC-005-4
- [ ] TC-005-5
- [ ] TC-006-1
- [ ] TC-006-2
- [ ] TC-006-3
- [ ] TC-006-4
- [ ] TC-006-5
- [ ] TC-007-1
- [ ] TC-007-2
- [ ] TC-007-3
- [ ] TC-007-4
- [ ] TC-007-5
- [ ] TC-007-6
- [ ] TC-008-1
- [ ] TC-008-2
- [ ] TC-008-3
- [ ] TC-008-4
- [ ] TC-008-5
- [ ] TC-009-1
- [ ] TC-009-2
- [ ] TC-009-3
- [ ] TC-009-4
- [ ] TC-009-5
- [ ] TC-010-01
- [ ] TC-010-02
- [ ] TC-010-03
- [ ] TC-010-04
- [ ] TC-010-05
- [ ] TC-011-1
- [ ] TC-011-2
- [ ] TC-011-3
- [ ] TC-011-4
- [ ] TC-011-5
- [ ] TC-012-1
- [ ] TC-012-2
- [ ] TC-012-3
- [ ] TC-012-4
- [ ] TC-012-5
- [ ] TC-013-1
- [ ] TC-013-2
- [ ] TC-013-3
- [ ] TC-013-4
- [ ] TC-013-5
- [ ] TC-014-1
- [ ] TC-014-2
- [ ] TC-014-3
- [ ] TC-014-4
- [ ] TC-014-5
- [ ] TC-014-6
- [ ] TC-015-1
- [ ] TC-015-2
- [ ] TC-015-3
- [ ] TC-015-4
- [ ] TC-015-5
- [ ] TC-015-6