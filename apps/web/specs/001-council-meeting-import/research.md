# Research: Council Meeting Notes Import & Construction Tracking

**Phase**: 0 — Pre-design research
**Date**: 2026-06-25
**Feature**: specs/001-council-meeting-import/spec.md

---

## Decision 1: PDF Text Extraction Library

**Decision**: `pdf-extract` crate (v0.7+)

**Rationale**: Pure Rust with zero native dependencies — no system libraries to ship or compile. Built on `lopdf` under the hood, reliably handles digitally-produced government PDFs. Extracts text with font-size and position metadata, giving sufficient signal to distinguish section headings from body text via font-size heuristics. This pairs well with the spec's section-heading + keyword classification approach.

**Alternatives considered**:
- `lopdf` alone: Too low-level — no text extraction API, requires manual coordinate parsing
- `pdfium-render`: Excellent fidelity, but requires shipping the PDFium native binary (~40 MB)
- `mupdf-sys`: Requires compiling MuPDF C library; unjustified complexity for digitally-produced PDFs

**Caveats**: If Montreal PDFs use unusual font embedding or complex column layouts, heading detection may require tuning. Keyword matching on flat extracted text remains reliable even if heading structure is imperfect. Upgrade path to `pdfium-render` if fidelity proves insufficient.

---

## Decision 2: Storage Engine

**Decision**: SQLite via `sqlx` (v0.8+) with WAL mode

**Rationale**: Zero operational overhead — the database is a single file, mountable as a Railway persistent volume. `sqlx` supports SQLite first-class with async, compile-time query checking, and `sqlx migrate`. WAL mode handles concurrent reads from the import scheduler and web server without contention. Perfectly matched to hundreds-to-low-thousands of records with low traffic.

**Alternatives considered**:
- PostgreSQL: Correct choice if horizontal scaling (multiple writers) is ever needed. Migration path is straightforward since `sqlx migrate` works identically for both. Defer until needed.

---

## Decision 3: Background Polling Scheduler

**Decision**: `tokio::time::interval` loop, no extra crate

**Rationale**: At daily polling frequency, a spawned Tokio task with `tokio::time::interval(Duration::from_secs(86_400))` is exactly sufficient. The first tick fires immediately on startup, self-healing any missed run after a restart. No additional dependencies or cron syntax required. Trivially testable by injecting a shorter interval in tests.

**Alternatives considered**:
- `tokio-cron-scheduler`: Adds a crate and cron syntax with no benefit at daily frequency
- External scheduler (Railway cron → `POST /internal/trigger-import`): Valid runner-up for production observability. Note as future option if per-run visibility in Railway's dashboard becomes desirable.

---

## Decision 4: Admin Authentication

**Decision**: HTTP Basic Auth via Axum middleware; credentials from environment variables

**Rationale**: Simple, no session infrastructure needed, standard browser support. Credentials (`ADMIN_USER`, `ADMIN_PASSWORD_HASH`) set at deployment time. Password stored as a bcrypt hash to avoid plaintext env vars. The Axum `tower` middleware layer checks the `Authorization` header on all `/admin/*` routes.

**Alternatives considered**:
- Session-based auth: Correct for a multi-user admin portal. Over-engineered for a single administrator checking import logs.

---

## Decision 5: E2E Testing Framework

**Decision**: pytest (as specified by user), with `httpx` for HTTP assertions

**Rationale**: pytest is a mature Python testing framework. `httpx` provides a clean async-capable HTTP client for asserting against the running Rust server. The E2E suite lives in `tests/e2e/` and treats the server as a black box. A `conftest.py` fixture starts the compiled server binary against a test SQLite database, runs the tests, then stops it.

**Toolchain**: `pytest`, `httpx`, `pytest-asyncio` (for async test support)

---

## New Dependencies for Cargo.toml

| Crate | Version | Purpose |
|-------|---------|---------|
| `reqwest` | 0.12 | Fetch HTML index page and download PDFs from Montreal portal |
| `scraper` | 0.22 | Parse HTML index to extract meeting PDF links |
| `pdf-extract` | 0.7 | Extract text content from PDF agendas |
| `sqlx` | 0.8 | SQLite async ORM with compile-time query checking and migrations |
| `lettre` | 0.11 | Send email alerts via SMTP |
| `chrono` | 0.4 | Date/time types for meeting dates and import timestamps |
| `regex` | 1 | Pattern matching for keyword classification rules |
| `serde_json` | 1 | JSON serialization for internal config |
| `bcrypt` | 0.15 | Admin password hashing |
