# Quickstart & Validation Guide

**Feature**: Council Meeting Notes Import & Construction Tracking
**Date**: 2026-06-25

This guide documents how to run and validate the feature end-to-end. It is not a tutorial — see [data-model.md](data-model.md) for schema details and [contracts/routes.md](contracts/routes.md) for route contracts.

---

## Prerequisites

- Rust stable toolchain (`rustup update stable`)
- `sqlx-cli`: `cargo install sqlx-cli --features sqlite`
- Python 3.11+ with pip (for E2E tests)
- An SMTP server or Mailtrap account (for alert testing)

---

## Setup

```bash
# 1. Create the SQLite database directory
mkdir -p data

# 2. Set environment variables (copy and fill in)
export DATABASE_URL="sqlite:./data/shovelsup.db"
export PORTAL_URL="https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL"
export POLLING_INTERVAL_SECS="86400"       # 24 hours; set lower for local testing
export ADMIN_USER="admin"
export ADMIN_PASSWORD_HASH="<bcrypt hash — generate with: htpasswd -bnBC 12 '' <password> | tr -d ':\n'>"
export SMTP_HOST="smtp.mailtrap.io"
export SMTP_PORT="587"
export SMTP_USER="<mailtrap user>"
export SMTP_PASSWORD="<mailtrap password>"
export ALERT_EMAIL_TO="you@example.com"
export ALERT_FAILURE_THRESHOLD="3"

# 3. Run database migrations
sqlx migrate run

# 4. Build and start the server
cargo run
# Server starts on http://localhost:3000
```

---

## Rust Unit & Integration Tests

```bash
cargo test
```

Key test modules to verify:
- `services::classifier` — section heading and keyword matching rules
- `services::extractor` — structured field extraction from sample agenda text
- `services::importer` — deduplication logic (dossier number vs. address fallback)
- `db` — CRUD operations against an in-memory SQLite database

---

## E2E Tests (pytest)

```bash
cd tests/e2e
pip install -r requirements.txt
pytest -v
```

The `conftest.py` fixture starts the compiled server binary against a temporary SQLite database, then tears it down after the session. Tests treat the server as a black box.

**requirements.txt**:
```
pytest>=8.0
httpx>=0.27
pytest-asyncio>=0.23
```

---

## Validation Scenarios

### SC-001: New meetings appear within 24 hours

1. The scheduler fires immediately on startup (first `interval.tick()`)
2. Confirm the portal HTML index is fetched and any new PDF links are queued
3. **Expected**: Within the first poll cycle, any meetings published since the last import appear in `/projects`

### SC-002: 100% of discovered meetings produce a log entry

1. Trigger an import of a known meeting PDF (either real or a test fixture)
2. **Expected**: One `ImportLog` entry created with `outcome = success` and `items_extracted > 0`, OR `outcome = no_items` — never a silent absence

### SC-003: Project reachable in 2 clicks

1. Start at `GET /` (homepage)
2. Click the Permits / Permis navigation link → arrives at `GET /projects`
3. Click any project row → arrives at `GET /projects/:id`
4. **Expected**: Full project detail reached in exactly 2 clicks from the homepage

### SC-004: Project list loads within 2 seconds (load test)

Verify with `wrk` (install: `brew install wrk` or `apt install wrk`):

```bash
# Seed 50+ projects first, then:
wrk -t2 -c10 -d10s http://localhost:3000/projects
```

**Expected**: Median latency < 2000 ms and no errors reported. Also verify with a single request:

```bash
time curl -s http://localhost:3000/projects > /dev/null
# Expected: real time < 2.000s
```

### SC-005: No duplicates on re-import

1. Import a meeting PDF once; note the project count
2. Import the same PDF again (or reset meeting status to `pending` and re-trigger)
3. **Expected**: Project count unchanged; no duplicate `ConstructionProject` records

### SC-006: Bilingual UI

1. `curl -H "Accept-Language: fr" http://localhost:3000/projects`
2. **Expected**: Response HTML contains French navigation labels (`Permis`, `Conseil`)
3. `curl -H "Accept-Language: en" http://localhost:3000/projects`
4. **Expected**: Response HTML contains English labels (`Permits`, `Council`)

### Admin auth gate (FR-013)

1. `curl http://localhost:3000/admin/imports` — **Expected**: HTTP 401
2. `curl -u admin:<password> http://localhost:3000/admin/imports` — **Expected**: HTTP 200

### SC-007: Project findable by address

1. `GET /projects?q=<street+address>` with a known project address
2. **Expected**: At least one matching project in the response within 3 seconds

### Email alert (manual test)

1. Set `ALERT_FAILURE_THRESHOLD=1` and point `DATABASE_URL` at a fresh database
2. Configure the portal URL to a non-existent host (or disconnect network)
3. Wait for the first poll cycle to fail
4. **Expected**: Email received at `ALERT_EMAIL_TO` describing the unreachable portal

---

## pytest E2E Test Coverage

| Test file | Scenarios covered |
|-----------|-------------------|
| `test_projects.py` | SC-003, SC-005, SC-007: list loads, language switching, address search |
| `test_import.py` | SC-001, SC-002, SC-004: import pipeline, deduplication, log entries |
| `test_admin.py` | SC-006: auth gate, import log display, freshness warning |
