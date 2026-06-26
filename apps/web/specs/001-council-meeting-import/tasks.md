# Tasks: Council Meeting Notes Import & Construction Tracking

**Input**: Design documents from `specs/001-council-meeting-import/`

**Prerequisites**: plan.md ✓, spec.md ✓, research.md ✓, data-model.md ✓, contracts/routes.md ✓, quickstart.md ✓

**Tests**: E2E tests written in pytest as requested. Rust integration tests required by project testing standards (one per route, in `tests/` crate). Cargo unit tests included inline for service modules with complex logic (classifier, extractor).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1–US5)

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add dependencies and create the database migration before any code is written.

- [ ] T001 Add new dependencies to Cargo.toml: reqwest 0.12, scraper 0.22, pdf-extract 0.7, sqlx 0.8 (sqlite + runtime-tokio + migrate + chrono features), lettre 0.11, chrono 0.4, regex 1, bcrypt 0.15, serde_json 1
- [ ] T002 Create migrations/001_initial.sql with full schema from data-model.md (meetings, construction_projects, project_decisions, import_log, classification_rules tables + default classification rule seed rows as listed in data-model.md)
- [ ] T003 [P] Create src/db/mod.rs: SqlitePool initialisation with WAL mode pragma, run sqlx migrations on startup
- [ ] T004 [P] Create tests/e2e/requirements.txt (pytest>=8.0, httpx>=0.27, pytest-asyncio>=0.23) and tests/e2e/conftest.py skeleton (server process fixture that starts the compiled binary against a temp SQLite DB and tears it down after the session)

---

## Phase 2: ADRs & Architecture Documentation

**Purpose**: Record all significant technical decisions and create living architecture diagrams before any implementation begins.

**⚠️ CRITICAL**: ADR tasks T005–T007 MUST be complete before any E2E test tasks. Architecture docs T008–T010 MUST be complete before opening a PR.

- [ ] T005 [P] Create docs/adr/001-e2e-tooling-pytest-httpx.md: document the decision to use pytest + httpx for E2E testing; include alternatives considered (cargo integration tests only, playwright), rationale (black-box HTTP testing of a Rust binary, pytest ecosystem), and consequences
- [ ] T006 [P] Create docs/adr/002-sqlite-storage.md: document the decision to use SQLite via sqlx; include alternatives (PostgreSQL), rationale (zero ops overhead, Railway volume mount, scale fit), and migration path to PostgreSQL if horizontal scaling is needed
- [ ] T007 [P] Create docs/adr/003-http-basic-auth.md: document the decision to use HTTP Basic Auth with bcrypt-hashed env-var credentials for the admin section; include alternatives (session-based auth, OAuth2), rationale (single administrator, no session infrastructure needed), and consequences
- [ ] T008 [P] Create docs/architecture/c4-system-context.md: Level 1 C4 system context diagram (PlantUML or Mermaid) showing ShovelsUp, its users (Resident, Journalist, Administrator), and external systems (Montreal City Council Portal, SMTP Email)
- [ ] T009 [P] Create docs/architecture/c4-container.md: Level 2 C4 container diagram showing the Axum Web Server container, the SQLite Database, the Import Scheduler (background task within the server), and the Montreal Portal as an external system; annotate each container with technology and responsibility
- [ ] T010 [P] Create docs/architecture/erd.md: entity relationship diagram (Mermaid erDiagram or equivalent) matching the schema in data-model.md, showing Meeting, ConstructionProject, ProjectDecision, ImportLog, ClassificationRule entities with cardinalities and FK relationships

---

## Phase 3: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [ ] T011 Update src/main.rs: extend AppState to hold SqlitePool, call db::init() at startup, run migrations before serving
- [ ] T012 Create src/auth.rs: Axum middleware that reads Authorization: Basic header on /admin/* routes, compares against ADMIN_USER + ADMIN_PASSWORD_HASH env vars (bcrypt verify), returns 401 with WWW-Authenticate: Basic realm="ShovelsUp Admin" header on failure
- [ ] T013 [P] Create src/models/meeting.rs: Meeting struct (all fields from data-model.md), sqlx FromRow derive, insert/find-by-reference-number/update-status queries
- [ ] T014 [P] Create src/models/project.rs: ConstructionProject struct, sqlx FromRow derive, insert/find-by-dossier/find-by-address/update-status queries, index-backed borough + status filters
- [ ] T015 [P] Create src/models/decision.rs: ProjectDecision struct with source_url field (meeting PDF URL, for FR-012 linkback), sqlx FromRow derive, insert query, find-by-project-id ordered by decided_at
- [ ] T016 [P] Create src/models/import_log.rs: ImportLog struct, sqlx FromRow derive, insert query with outcome values (success/failure/no_items), find-recent ordered by attempt_at desc
- [ ] T017 [P] Create src/models/classification_rule.rs: ClassificationRule struct, sqlx FromRow derive, load-all-enabled query returning section_heading and keyword rules separately (used by classifier at startup)
- [ ] T018 Update src/routes/mod.rs: register /projects route group (projects.rs), /admin route group behind auth middleware (admin.rs), expose new handler modules

**Checkpoint**: Database schema created, AppState updated with pool, auth middleware ready, all models queryable — user story implementation can now begin.

---

## Phase 4: User Story 1 — Browse Recent Construction Decisions (Priority: P1) 🎯 MVP

**Goal**: Public users can visit the site and see a browseable, bilingual list of construction projects. Selecting one shows full details.

**Independent Test**: Seed the database with 3 construction projects (different boroughs, one with 2 decisions). Verify the list page shows all 3, the detail page shows the timeline, and language switching between EN and FR works.

### E2E Tests for User Story 1

- [ ] T019 [P] [US1] Write tests/e2e/test_projects.py: test GET /projects returns HTTP 200 with seeded projects visible in response body
- [ ] T020 [P] [US1] Write tests/e2e/test_projects.py: test GET /projects/:id returns HTTP 200 with project address and decision type in response body; test GET /projects/99999 returns HTTP 404
- [ ] T021 [P] [US1] Write tests/e2e/test_projects.py: test Accept-Language: fr header causes French nav labels (Permis, Conseil) to appear; test Accept-Language: en causes English labels

### Rust Integration Tests for User Story 1

- [ ] T022 [P] [US1] Create tests/projects_test.rs: Rust integration test for GET /projects returns 200 and renders project list; GET /projects with empty DB returns 200 with empty-state message
- [ ] T023 [P] [US1] Create tests/projects_test.rs: Rust integration test for GET /projects/:id with seeded project returns 200; GET /projects/99999 returns 404

### Implementation for User Story 1

- [ ] T024 [P] [US1] Create templates/projects/list.html: extends base.html, renders project list table (address, borough, project_type, current_status, decided_at) with EN/FR string keys passed from route context; empty-state message when no projects
- [ ] T025 [P] [US1] Create templates/projects/detail.html: extends base.html, renders project header (address, borough, type, current status) + chronological decision timeline (decided_at, decision_type, conditions, link to source PDF); all visible text from EN/FR context variables
- [ ] T026 [US1] Create src/routes/projects.rs: GET /projects handler — query construction_projects joined with latest project_decisions, build EN/FR string map for both languages, detect language via Accept-Language header (reuse detect_lang), pass translated strings + project list to list.html template
- [ ] T027 [US1] Add GET /projects/:id handler in src/routes/projects.rs — query ConstructionProject by id (404 if absent), fetch all ProjectDecisions ordered by decided_at, pass EN/FR string map, render detail.html with timeline
- [ ] T028 [US1] Register /projects and /projects/:id routes in src/routes/mod.rs; verify cargo build passes

**Checkpoint**: `GET /projects` and `GET /projects/:id` work with seeded data. Language switching confirmed. E2E tests T019–T021 and Rust integration tests T022–T023 pass.

---

## Phase 5: User Story 2 — Automatic Discovery and Import (Priority: P2)

**Goal**: The system polls the Montreal portal on a daily schedule (configurable via env var), downloads new PDF agendas, classifies construction items, and stores them. Failures are logged; repeated failures trigger an email alert.

**Independent Test**: Point the crawler at a local HTTP fixture serving an HTML index with one PDF link. Trigger the scheduler manually. Verify one Meeting record, ≥1 ConstructionProject records, and one ImportLog (outcome=success) are created. Re-trigger — verify no duplicate projects. Verify that a PDF with no construction items produces an ImportLog with outcome=no_items.

### E2E Tests for User Story 2

- [ ] T029 [P] [US2] Write tests/e2e/test_import.py: with a fixture HTML index and PDF, trigger import; assert ImportLog outcome=success and item count > 0; assert ConstructionProject records exist
- [ ] T030 [P] [US2] Write tests/e2e/test_import.py: trigger same import twice; assert ConstructionProject count unchanged (deduplication by dossier number or normalized address)

### Rust Unit Tests for User Story 2

- [ ] T031 [P] [US2] Cargo unit tests inline in src/services/classifier.rs: test section heading detection for "Urbanisme", "Permis et dérogations"; test keyword matching for "dérogation", "zonage"; test item under heading with no keyword is still classified; test item with no heading match but keyword match is classified
- [ ] T032 [P] [US2] Cargo unit tests inline in src/services/extractor.rs: test address normalisation (number + street + borough); test dossier number extraction from city reference format; test decision_type extraction from French vote language (approuvé, ajourné, refusé)

### Rust Integration Tests for User Story 2

- [ ] T033 [P] [US2] Create tests/importer_test.rs: Rust integration test for importer deduplication — import same fixture twice, assert ConstructionProject count is 1; assert ProjectDecision count is 1 (no duplicate decisions for same meeting)

### Implementation for User Story 2

- [ ] T034 [P] [US2] Create src/services/crawler.rs: fetch HTML index at PORTAL_URL with reqwest, parse with scraper to extract meeting reference numbers and PDF links not yet present in meetings table, return list of (reference_number, pdf_url) pairs
- [ ] T035 [P] [US2] Create src/services/pdf_parser.rs: download PDF bytes from url with reqwest, extract full text with pdf-extract preserving font-size metadata for heading detection, return structured Vec<PdfPage> with text blocks and relative font sizes
- [ ] T036 [P] [US2] Create src/services/classifier.rs: load enabled ClassificationRule rows from DB at startup (section_heading and keyword rules separately), expose classify(pages: &[PdfPage]) -> Vec<AgendaItem> — an item qualifies if it is under a matching section heading OR contains a matching keyword; include inline unit tests (see T031)
- [ ] T037 [P] [US2] Create src/services/extractor.rs: given raw French agenda text, extract normalized_address, dossier_number, borough, project_type, decision_type, and conditions; include inline unit tests (see T032)
- [ ] T038 [US2] Create src/services/importer.rs: orchestrate pipeline — for each new meeting: set status=processing, call pdf_parser, classifier, extractor; for each extracted item apply dedup logic (match dossier → match address → create new); insert ProjectDecision with source_url set to meeting pdf_url (FR-012); when classifier returns 0 items write ImportLog with outcome=no_items and item_count=0 (SC-002 no-silent-failure); update Meeting status=imported + item_count; on any error set status=failed + error_message; write ImportLog entry regardless of outcome
- [ ] T039 [P] [US2] Create src/services/mailer.rs: lettre SMTP client configured from env vars (SMTP_HOST, SMTP_PORT, SMTP_USER, SMTP_PASSWORD, ALERT_EMAIL_TO); send_alert(subject, body) function; unit test with mock transport asserting recipient and subject
- [ ] T040 [US2] Create src/services/scheduler.rs: read POLLING_INTERVAL_SECS env var (default: 86400); tokio::time::interval loop with that duration; on each tick call importer for any pending/new meetings; track consecutive_failures counter; when counter reaches ALERT_FAILURE_THRESHOLD (env var, default: 3) call mailer::send_alert and reset counter; reset counter on success
- [ ] T041 [US2] Update src/main.rs: pass db pool clone + smtp config to scheduler; tokio::spawn(scheduler::run(pool, config)) before axum::serve

**Checkpoint**: Scheduler fires on startup, imports from fixture/real portal, deduplication confirmed, no_items path logged. ImportLog populated. E2E tests T029–T030 and Rust integration test T033 pass.

---

## Phase 6: User Story 3 — Filter and Search Projects (Priority: P3)

**Goal**: Users can filter the project list by borough and date range, and search by address or description keywords.

**Independent Test**: Seed 5 projects across 3 boroughs and 2 date ranges. Verify borough filter returns only matching borough, date filter returns only meetings in range, text search returns the matching project within 3 seconds, and an impossible filter returns the no-results state.

### E2E Tests for User Story 3

- [ ] T042 [P] [US3] Write tests/e2e/test_projects.py: GET /projects?borough=Rosemont-La+Petite-Patrie returns only projects in that borough
- [ ] T043 [P] [US3] Write tests/e2e/test_projects.py: GET /projects?from=2024-01-01&to=2024-06-30 returns only projects with decided_at in range
- [ ] T044 [P] [US3] Write tests/e2e/test_projects.py: GET /projects?q=rue+Sherbrooke returns project with matching address; response arrives within 3 seconds
- [ ] T045 [P] [US3] Write tests/e2e/test_projects.py: GET /projects?borough=NonExistent returns 200 with empty list message (not 404)

### Rust Integration Tests for User Story 3

- [ ] T046 [P] [US3] Add to tests/projects_test.rs: Rust integration test GET /projects?borough= with seeded multi-borough data returns only matching borough projects; GET /projects?q= with seeded address returns matching project

### Implementation for User Story 3

- [ ] T047 [US3] Update GET /projects handler in src/routes/projects.rs: build SQL query dynamically appending WHERE borough = ?, WHERE decided_at >= ?, WHERE decided_at <= ?, WHERE normalized_address LIKE ? clauses from query params; validate date format; pass applied filter values back to template for form pre-fill
- [ ] T048 [US3] Update templates/projects/list.html: add filter form (borough select populated from distinct DB values, from/to date inputs, text search box with EN/FR labels from context); show active filter chips; no-results message with "clear filters" link (all filter UI text from context variables)

**Checkpoint**: All filter and search combinations work. No-results state renders correctly. E2E tests T042–T045 and Rust integration test T046 pass.

---

## Phase 7: User Story 4 — Monitor Import Health via Admin Dashboard (Priority: P4)

**Goal**: An authenticated administrator can view import history, spot failures, and confirm data freshness via a protected web page.

**Independent Test**: Seed one success and one failure ImportLog entry. Log in as admin (valid credentials). Verify the dashboard shows both log entries with all fields. Verify unauthenticated request returns 401. Seed a success >48h ago; verify freshness warning appears.

### E2E Tests for User Story 4

- [ ] T049 [P] [US4] Write tests/e2e/test_admin.py: GET /admin/imports without credentials returns HTTP 401 with WWW-Authenticate header
- [ ] T050 [P] [US4] Write tests/e2e/test_admin.py: GET /admin/imports with valid Basic Auth credentials returns HTTP 200; seeded ImportLog entries visible in response body
- [ ] T051 [P] [US4] Write tests/e2e/test_admin.py: when most recent successful ImportLog is >48h old, response body contains freshness warning text

### Rust Integration Tests for User Story 4

- [ ] T052 [P] [US4] Create tests/admin_test.rs: Rust integration test GET /admin/imports without Authorization header returns 401; GET /admin/imports with valid Basic credentials returns 200 and ImportLog data

### Implementation for User Story 4

- [ ] T053 [US4] Create src/routes/admin.rs: GET /admin (redirect 302 to /admin/imports); GET /admin/imports handler — query ImportLog ordered by attempt_at desc, compute hours_since_last_success, build EN/FR string map for all admin UI labels, pass everything to template
- [ ] T054 [US4] Create templates/admin/dashboard.html: summary row (total meetings, total projects, last successful import datetime); freshness warning banner when >48h since last success; import log table (attempt_at, outcome, meeting reference, items_extracted, duration_ms, error_detail); ALL visible text passed as Minijinja context variables — no hardcoded English or French strings in the template
- [ ] T055 [US4] Wire /admin routes behind auth middleware in src/routes/mod.rs: nest /admin router under auth::layer(); verify unauthenticated requests receive 401

**Checkpoint**: Admin dashboard accessible with credentials, blocked without. Import log shows real data. Freshness warning triggers correctly. E2E tests T049–T051 and Rust integration test T052 pass.

---

## Phase 8: User Story 5 — Track Project Status Across Meetings (Priority: P5)

**Goal**: When a project has appeared in multiple meetings, its detail page shows a chronological timeline of all decisions.

**Independent Test**: Seed one ConstructionProject with ProjectDecision records from two different meetings (deferred then approved). Verify the detail page renders both in order with current status = approved.

### E2E Tests for User Story 5

- [ ] T056 [P] [US5] Write tests/e2e/test_projects.py: GET /projects/:id for a project with 2 decisions (deferred → approved) returns both in chronological order; current_status shows approved

### Rust Integration Tests for User Story 5

- [ ] T057 [P] [US5] Add to tests/projects_test.rs: Rust integration test GET /projects/:id with 2 seeded decisions returns decisions in decided_at ASC order; most recent decision_type matches current_status

### Implementation for User Story 5

- [ ] T058 [US5] Update GET /projects/:id query in src/routes/projects.rs: JOIN project_decisions ordered by decided_at ASC, pass full timeline list to template; current_status is derived from the most recent decision_type
- [ ] T059 [US5] Update templates/projects/detail.html: render timeline section as vertical list of decision events (meeting date, decision_type badge, conditions if present, link to source PDF); clearly mark the most recent decision as current status; all labels from EN/FR context variables

**Checkpoint**: Multi-meeting project timelines render correctly. E2E test T056 and Rust integration test T057 pass.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Verification, observability, and bilingual completeness across all stories.

- [ ] T060 [P] Verify classification_rules seed rows are inserted by migration 001_initial.sql; add a startup log line in src/services/classifier.rs listing how many enabled section_heading and keyword rules were loaded
- [ ] T061 [P] Add structured tracing spans throughout the import pipeline in src/services/ (importer, crawler, pdf_parser) using the existing tracing crate: log meeting reference, item count, duration, and outcome on each import cycle
- [ ] T062 [P] Audit all Minijinja templates (templates/projects/list.html, detail.html, templates/admin/dashboard.html) for EN/FR string completeness — verify every user-visible string is a context variable with both language variants provided by the route handler; fix any hardcoded strings found
- [ ] T063 Run all 7 quickstart.md validation scenarios (SC-001 through SC-007); for SC-004 run `wrk -t2 -c10 -d10s http://localhost:3000/projects` and confirm median response time < 2s; fix any gaps before marking complete
- [ ] T064 [P] Update CLAUDE.md commands section: add `sqlx migrate run`, pytest invocation (`cd tests/e2e && pytest -v`), and bcrypt password hash generation (`htpasswd -bnBC 12 "" <password> | tr -d ':\n'`)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — start immediately
- **Phase 2 (ADRs & Docs)**: Depends on Phase 1 — MUST complete T005–T007 (ADRs) before any E2E tests; MUST complete T008–T010 before PR
- **Phase 3 (Foundational)**: Depends on Phase 1 — **BLOCKS all user stories**
- **Phase 4 (US1)**: Depends on Phase 2 (ADRs) and Phase 3 — MVP deliverable
- **Phase 5 (US2)**: Depends on Phase 3 — can start in parallel with US1 after Phase 3
- **Phase 6 (US3)**: Depends on Phase 4 (US1) — extends the project list page
- **Phase 7 (US4)**: Depends on Phase 3 — can run in parallel with US1/US2 after Phase 3
- **Phase 8 (US5)**: Depends on Phase 4 (US1) — extends the project detail page
- **Phase 9 (Polish)**: Depends on all desired stories being complete

### User Story Dependencies

- **US1 (P1)**: Needs Phase 2 (ADRs) and Phase 3 — start after both
- **US2 (P2)**: Needs Phase 3 only — start in parallel with US1
- **US3 (P3)**: Extends US1 (adds filters to the list page built in US1)
- **US4 (P4)**: Needs Phase 3 only — start in parallel with US1/US2
- **US5 (P5)**: Extends US1 (adds timeline to the detail page built in US1)

### Within Each User Story

- E2E tests first (write them, run `pytest -v` and confirm they fail before implementation)
- Rust integration tests next (write them, run `cargo test` and confirm they fail)
- Templates before route handlers (handlers pass context to templates)
- Services before handlers for US2
- Core implementation before wiring into router

---

## Parallel Opportunities

### After Phase 3 completes, launch in parallel

```
Developer A: Phase 4 (US1) → Phase 6 (US3) → Phase 8 (US5)
Developer B: Phase 5 (US2) → Phase 7 (US4)
```

### Within Phase 5 (US2) — services are independent files

```
Parallel: T034 (crawler) + T035 (pdf_parser) + T036 (classifier) + T037 (extractor) + T039 (mailer) + T031 (classifier tests) + T032 (extractor tests)
Then sequential: T038 (importer, orchestrates all above) → T040 (scheduler) → T041 (wire into main)
```

### Within each phase — E2E and Rust integration tests are parallel

```
Parallel: T019 + T020 + T021 (US1 E2E tests)
Parallel: T022 + T023 (US1 Rust integration tests)
Parallel: T024 + T025 (US1 templates — different files)
```

### Phase 2 tasks are all parallel

```
Parallel: T005 + T006 + T007 + T008 + T009 + T010 (all different files)
```

---

## Implementation Strategy

### MVP First (User Story 1 only)

1. Complete Phase 1: Setup
2. Complete Phase 2: ADRs & Architecture Documentation (MUST before E2E)
3. Complete Phase 3: Foundational (CRITICAL — blocks all stories)
4. Complete Phase 4: User Story 1 (browse projects)
5. **STOP and VALIDATE**: Run `pytest -v tests/e2e/test_projects.py` + `cargo test`, verify bilingual rendering
6. Demo with seeded data if ready

### Incremental Delivery

1. Phase 1–3 → foundation + ADRs ready
2. Phase 4 (US1) → browse projects MVP ← **first public release possible**
3. Phase 5 (US2) → import pipeline live → real data appears
4. Phase 6 (US3) → filtering and search
5. Phase 7 (US4) → admin health dashboard
6. Phase 8 (US5) → multi-meeting project timelines
7. Phase 9 (Polish) → observability + bilingual audit

### Parallel Team Strategy

With two developers after Phase 3:
- Dev A: US1 → US3 → US5 (public-facing browsing flows)
- Dev B: US2 → US4 (import pipeline + admin)

Stories integrate naturally: US2 populates the data US1 displays.

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks in same phase
- [Story] label maps each task to a specific user story for traceability
- Write E2E tests first, then Rust integration tests, confirm both fail before implementing
- Each user story phase ends with a named checkpoint — validate before proceeding
- Commit after each checkpoint at minimum
- Avoid cross-story file edits mid-phase; complete the current story's tasks before branching into the next
- ADRs live in `docs/adr/`; architecture diagrams in `docs/architecture/`
