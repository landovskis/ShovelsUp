# HTTP Route Contracts

**Feature**: Council Meeting Notes Import & Construction Tracking
**Server**: Axum 0.7, port 3000
**Language negotiation**: `Accept-Language` header; defaults to `en`

---

## Public Routes (no authentication)

### `GET /projects`

Returns the construction project list filtered by optional query parameters.

**Query parameters**:

| Parameter | Type | Description |
|-----------|------|-------------|
| `borough` | string | Filter by Montreal arrondissement (exact match, case-insensitive) |
| `from` | date (`YYYY-MM-DD`) | Earliest meeting date to include |
| `to` | date (`YYYY-MM-DD`) | Latest meeting date to include |
| `q` | string | Free-text search against address and project description |
| `lang` | `en` \| `fr` | Override Accept-Language header for language |

**Default ordering**: Most recent `decided_at` date first.

**Response**: HTML page rendering the project list in the requested language.

**Empty state**: Page renders with a "no results" message and a prompt to adjust filters.

---

### `GET /projects/:id`

Returns full detail for a single construction project, including its complete decision timeline.

**Path parameters**:

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | ConstructionProject primary key |

**Response**: HTML page with project details (address, borough, project type, current status) and a chronological list of `ProjectDecision` records (meeting date, decision type, conditions, link to source PDF).

**Not found**: Returns HTTP 404 with a user-friendly error page.

---

## Admin Routes (HTTP Basic Auth required)

All `/admin/*` routes require a valid `Authorization: Basic <credentials>` header. Credentials are compared against `ADMIN_USER` and `ADMIN_PASSWORD_HASH` environment variables. Invalid or missing credentials return HTTP 401 with a `WWW-Authenticate: Basic realm="ShovelsUp Admin"` header (triggering browser credential prompt).

### `GET /admin`

Redirects (HTTP 302) to `/admin/imports`.

---

### `GET /admin/imports`

Displays the import log: all `ImportLog` records ordered by `attempt_at` descending.

**Response**: HTML admin dashboard showing:
- Summary: last successful import time, total meetings imported, total projects tracked
- Freshness warning if the most recent successful import is older than 48 hours
- Table: attempt timestamp, outcome (`success` / `failure` / `no_items`), meeting reference, items extracted, duration, error detail (on failure)

---

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `DATABASE_URL` | Yes | SQLite file path, e.g. `sqlite:./data/shovelsup.db` |
| `PORTAL_URL` | Yes | Montreal City Council meeting index URL (HTML page listing meeting PDFs) |
| `POLLING_INTERVAL_SECS` | No | Seconds between portal checks (default: 86400 — once per day) |
| `ADMIN_USER` | Yes | Admin username for HTTP Basic Auth |
| `ADMIN_PASSWORD_HASH` | Yes | bcrypt hash of the admin password |
| `SMTP_HOST` | Yes | SMTP server hostname for email alerts |
| `SMTP_PORT` | No | SMTP port (default: 587) |
| `SMTP_USER` | Yes | SMTP authentication username |
| `SMTP_PASSWORD` | Yes | SMTP authentication password |
| `ALERT_EMAIL_TO` | Yes | Recipient address for portal-unreachable alerts |
| `ALERT_FAILURE_THRESHOLD` | No | Consecutive failures before alerting (default: 3) |
| `RUST_LOG` | No | Log filter, e.g. `shovelsup_web=debug` |
