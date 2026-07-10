# ADR 005 — Docker Compose for Local Development

**Status**: Accepted
**Date**: 2026-07-04
**Feature**: specs/001-council-meeting-import

## Context

The web app now depends on a running Postgres instance (`sqlx::PgPool`, held on
`AppState` — see `src/lib.rs`) and a running Redis instance (`REDIS_URL`, used for rate
limiting) in addition to the compiled Rust binary. Previously a developer needed to
install and manage Postgres and Redis locally by hand, with no standard way to reproduce
the same environment across machines or CI.

Options considered:

| Option | Description |
|--------|-------------|
| Manual local install | Each developer installs Postgres/Redis natively |
| Docker Compose | `docker-compose.yml` orchestrates app + Postgres + Redis containers |
| Full Kubernetes/Helm | Container orchestration platform for local + prod |

## Decision

Use **Docker Compose** (`apps/web/docker-compose.yml` and `apps/web/Dockerfile`) to run
the app, Postgres 16, and Redis 7 as three services on a shared network for local
development.

- `Dockerfile` is a two-stage build: `rust:1-bookworm` compiles the release binary,
  `debian:bookworm-slim` runs it. `templates/` and `static/` are copied into the runtime
  image since they are read from disk at runtime (not embedded). Migrations under
  `migrations/` are embedded into the binary at compile time via `sqlx::migrate!` and run
  automatically on startup, so no separate migration step or image content is needed.
- Postgres and Redis use official Alpine images with named volumes for persistence and
  healthchecks gating the app's startup (`depends_on: condition: service_healthy`). Both
  also publish host ports (`POSTGRES_HOST_PORT`, default 5434; `REDIS_HOST_PORT`, default
  6380 — non-default to avoid colliding with any native Postgres/Redis already running on
  a developer's machine) so `cargo check`/`cargo test` can run directly on the host
  against the compose stack, not just from inside the `app` container.
- Secrets and per-developer config (`JWT_SECRET`, `ADMIN_PASSWORD_HASH`,
  `ANTHROPIC_API_KEY`, etc.) are supplied via a gitignored `.env` file in `apps/web/`,
  documented in `.env.example`.

## Rationale

- Manual local installs drift between machines and don't match production topology
  (separate Postgres/Redis instances).
- Full Kubernetes/Helm is disproportionate for a single-service app at this stage; it
  adds operational overhead with no current multi-service or scaling requirement.
- Compose gives one command (`docker compose up`) to reproduce the full runtime
  dependency graph, and doubles as a lightweight reference for future production
  container deployment.

## Consequences

- **Docker required for local dev**: Contributors need Docker and Docker Compose
  installed; `cargo run` against a manually-run Postgres/Redis is still possible but is
  no longer the documented path.
- **Image rebuild on dependency change**: `docker compose build` must be re-run after
  `Cargo.toml`/`Cargo.lock` changes; there is no dependency-layer caching trick in the
  Dockerfile (kept simple — full source is copied and built in one layer).
- **Compile-time SQL verification via SQLx offline mode**: the app uses `sqlx::query!`/
  `query_scalar!`, which verify SQL against a live database schema at compile time. The
  Dockerfile therefore sets `SQLX_OFFLINE=true` and builds against the `.sqlx/` query
  cache checked into the repo, rather than requiring `DATABASE_URL` at image build time.
  Anyone changing a query must re-run `cargo sqlx prepare` (against a migrated dev
  database) and commit the updated `.sqlx/` files, or the next build — local offline or
  in Docker — will use stale query metadata.
- **Storage decision superseded**: this ADR assumes the current codebase's Postgres
  backend, not any SQLite backend described in an earlier ADR 002; ADR 002, if reinstated,
  should be marked Superseded by this one.
