# Implementation Plan: Council Meeting Notes Import & Construction Tracking

**Branch**: `feature/montreal-import-2` | **Date**: 2026-06-25 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-council-meeting-import/spec.md`

## Summary

Automatically discover, import, and surface Montreal City Council construction decisions to the public. The system polls the Montreal portal's HTML meeting index on a daily schedule, downloads linked PDF agendas, classifies construction-related items by section heading and keyword, and stores them in a SQLite database. A public Axum web interface lets residents and journalists browse, filter, and track projects across meetings. A password-protected admin page shows import history and freshness status. See [research.md](research.md) for all technology decisions.

## Technical Context

**Language/Version**: Rust stable (1.79+)

**Primary Dependencies**:
- Axum 0.7 + Tokio 1 (existing)
- Minijinja 2 (existing)
- `reqwest` 0.12 — HTTP client for portal HTML and PDF downloads
- `scraper` 0.22 — HTML parsing for meeting index page
- `pdf-extract` 0.7 — Pure-Rust PDF text extraction (built on `lopdf`)
- `sqlx` 0.8 with SQLite feature — async ORM, compile-time queries, migrations
- `lettre` 0.11 — SMTP email for unreachable-portal alerts
- `chrono` 0.4 — Date/time types
- `regex` 1 — Keyword classification matching
- `bcrypt` 0.15 — Admin password hashing

**Storage**: SQLite (file: `data/shovelsup.db`, WAL mode). See [data-model.md](data-model.md).

**Testing**:
- Unit tests: inline `#[cfg(test)]` modules in `src/services/classifier.rs` and `src/services/extractor.rs`
- Rust integration tests: `tests/projects_test.rs`, `tests/admin_test.rs`, `tests/importer_test.rs` (one per route group and per service boundary)
- E2E: `pytest` + `httpx` (Python, in `tests/e2e/`)

**Target Platform**: Linux server (Railway or equivalent), single instance

**Project Type**: Web service

**Performance Goals**: Project list response < 2 s; search response < 3 s; new meetings appear within 24 h of portal publication

**Constraints**: SQLite single-writer; polling interval daily; admin auth via HTTP Basic Auth

**Scale/Scope**: ~12 meetings/year, ~50–200 construction items per meeting, low concurrent traffic (public information site)

## Constitution Check

*Gate: verified against project governance standards before Phase 0 research.*

| Principle | Status | Evidence |
|-----------|--------|---------|
| ADR requirement — significant technical decisions must be documented in `docs/adr/` before implementation | ✅ Pass | Tasks T005–T007 create ADRs for E2E tooling, SQLite, and HTTP Basic Auth before any implementation begins |
| Living documentation — C4 diagrams (L1 + L2) and ERD in `docs/architecture/` must stay current; stale diagrams block PRs | ✅ Pass | Tasks T008–T010 create system context diagram, container diagram, and ERD as part of Phase 2 (before implementation) |
| Automated testing — unit, integration, and E2E tests mandatory for every feature; all three layers gate CI merge | ✅ Pass | Cargo unit tests in classifier and extractor services; Rust integration tests per route (`tests/projects_test.rs`, `tests/admin_test.rs`, `tests/importer_test.rs`); pytest E2E suite in `tests/e2e/` |

## Project Structure

### Documentation (this feature)

```text
specs/001-council-meeting-import/
├── plan.md          ← this file
├── research.md      ← technology decisions
├── data-model.md    ← schema and entity definitions
├── quickstart.md    ← validation guide
├── contracts/
│   └── routes.md    ← HTTP route contracts and env vars
├── checklists/
│   └── requirements.md
└── tasks.md         ← created by /speckit-tasks (not yet)
```

### Source Code

```text
src/
├── main.rs                    # Updated: add db pool, spawn scheduler task
├── auth.rs                    # New: HTTP Basic Auth middleware for /admin/*
├── routes/
│   ├── mod.rs                 # Updated: register /projects and /admin routes
│   ├── projects.rs            # New: GET /projects, GET /projects/:id
│   └── admin.rs               # New: GET /admin, GET /admin/imports
├── db/
│   └── mod.rs                 # New: SqlitePool init, run migrations
├── models/
│   ├── meeting.rs             # New: Meeting struct + sqlx queries
│   ├── project.rs             # New: ConstructionProject struct + queries
│   ├── decision.rs            # New: ProjectDecision struct + queries (includes source_url)
│   ├── import_log.rs          # New: ImportLog struct + queries
│   └── classification_rule.rs # New: ClassificationRule struct + load-all-enabled query
└── services/
    ├── crawler.rs             # New: fetch HTML index, extract PDF links
    ├── pdf_parser.rs          # New: download PDF, extract text via pdf-extract
    ├── classifier.rs          # New: section heading + keyword classification
    ├── extractor.rs           # New: parse address, dossier, decision from text
    ├── importer.rs            # New: orchestrate pipeline, dedup, write DB
    ├── scheduler.rs           # New: tokio::time::interval polling loop
    └── mailer.rs              # New: lettre SMTP alert on consecutive failures

migrations/
└── 001_initial.sql            # New: full schema (see data-model.md)

templates/
├── base.html                  # Existing (may need nav updates)
├── index.html                 # Existing
├── projects/
│   ├── list.html              # New: project list with filters
│   └── detail.html            # New: project detail + decision timeline
└── admin/
    └── dashboard.html         # New: import log table + freshness summary

tests/
├── projects_test.rs           # New: Rust integration tests for /projects routes
├── admin_test.rs              # New: Rust integration tests for /admin routes (auth + data)
├── importer_test.rs           # New: Rust integration tests for importer deduplication
└── e2e/
    ├── conftest.py            # New: server fixture, test DB setup/teardown
    ├── test_projects.py       # New: list, search, language, detail
    ├── test_import.py         # New: pipeline, dedup, log entries
    ├── test_admin.py          # New: auth gate, dashboard, freshness warning
    └── requirements.txt       # New: pytest, httpx, pytest-asyncio

docs/
├── adr/
│   ├── 001-e2e-tooling-pytest-httpx.md  # New: E2E tooling decision
│   ├── 002-sqlite-storage.md             # New: storage engine decision
│   └── 003-http-basic-auth.md            # New: admin auth strategy decision
└── architecture/
    ├── c4-system-context.md              # New: Level 1 C4 diagram
    ├── c4-container.md                   # New: Level 2 C4 diagram
    └── erd.md                            # New: entity relationship diagram
```

**Structure decision**: Existing `src/routes/mod.rs` is extended with new route modules. New `src/services/` and `src/models/` directories follow idiomatic Rust module layout. E2E tests are isolated under `tests/e2e/` as a Python project — separate from `cargo test` to keep Rust and Python toolchains independent.

## Complexity Tracking

No complexity violations. Constitution gates satisfied — see Constitution Check above.
