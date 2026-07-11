# Modular monolith: split `apps/web` into a Cargo workspace

## Context

`apps/web` is currently a single Rust crate (`shovelsup-web`) with folder-based
modules: `domain/`, `pipeline/` (with `extractor`, `fetcher`, `normalizer`,
`parser`, `redaction`, `resolver`, `scheduler`, `metrics`), `routes/`, `jobs/`,
`middleware/`, `config/`. Every module is declared `pub mod` in `lib.rs`, and
nothing stops any module from reaching into any other module's internals —
e.g. `routes/admin.rs` already imports `pipeline::parser::{ocr, orchestrate}`
directly, and there is no compiler-enforced distinction between a module's
public API and its implementation details.

Goal: general hygiene and onboarding clarity, not preparation for splitting
into separate deployed services. There is no near-term plan to run `domain`,
`pipeline`, or `web` as independent processes.

## Dependency graph (verified by grep of `use crate::` across `src/`)

- `domain` (`business_days.rs`, `review_queue.rs`): no dependency on
  `pipeline`, `routes`, `jobs`, `middleware`, or `config`. Talks to Postgres
  directly via `sqlx::PgPool`.
- `pipeline` (`extractor`, `fetcher`, `metrics`, `normalizer`, `parser`,
  `redaction`, `resolver`, `scheduler`): no dependency on `domain`. One
  internal cross-reference: `pipeline::extractor` imports
  `pipeline::{normalizer, redaction}` (all within `pipeline`).
- `jobs` (`sla_sweep.rs`, `public_search_refresh.rs`): plain functions taking
  `PgPool`, no dependency on `pipeline` or `domain`.
- `routes`, `middleware`, `config`, `lib.rs` (`AppState`, `app()`): the only
  layer that depends on both `domain` and `pipeline` (e.g.
  `routes/review_queue.rs` → `domain::review_queue`, `routes/admin.rs` →
  `pipeline::parser`).

`domain` and `pipeline` are siblings with no dependency on each other. Both
are consumed only by the web layer. This graph maps directly onto a
three-crate workspace with one dependency direction: `web` → `{domain,
pipeline}`.

## Design

### Workspace layout

```
apps/web/
  Cargo.toml              # [workspace] members = ["domain", "pipeline", "web"]
  domain/
    Cargo.toml             # shovelsup-domain
    src/
      lib.rs                # pub use business_days::*; pub use review_queue::*;
      business_days.rs
      review_queue.rs
  pipeline/
    Cargo.toml             # shovelsup-pipeline
    src/
      lib.rs                # curated pub re-exports; internal-only items become pub(crate)
      extractor/
      fetcher.rs
      metrics.rs
      normalizer/
      parser/
      redaction/
      resolver/
      scheduler.rs
  web/
    Cargo.toml             # shovelsup-web, [[bin]] shovelsup-web
    src/
      main.rs
      lib.rs                # AppState, app() — depends on shovelsup-domain, shovelsup-pipeline
      routes/
      jobs/
      middleware/
      config/
    tests/                  # moved from apps/web/tests/
  templates/                 # stays at apps/web root, referenced by web crate at runtime
  static/                    # stays at apps/web root
  migrations/                 # stays at apps/web root (sqlx migrate reads this path)
  .sqlx/                      # stays at apps/web root (offline query cache, workspace-wide)
  docker-compose.yml           # stays at apps/web root
  data/                        # stays at apps/web root
```

`apps/web/Cargo.toml` becomes a workspace manifest only (`[workspace]`
section, no `[package]`). The existing `[dependencies]` list gets
partitioned across the three new crate manifests according to what each
crate's code actually uses (e.g. `scraper`/`whatlang`/`encoding_rs` move to
`pipeline`; `axum`/`minijinja`/`tower-http` move to `web`; `sqlx`/`chrono`
are needed by all three and stay in each crate's own `Cargo.toml`, deduped
by Cargo's workspace dependency resolution).

### Boundary enforcement

Each crate's `lib.rs` re-exports only the items other crates are meant to
call (`pub use domain::review_queue::{confirm_candidate, reject_candidate,
ReviewQueueError};` etc.). Submodules that are purely implementation detail
today — e.g. `pipeline::extractor::schema`, `pipeline::extractor::validator`
— become `pub(crate)` inside the `pipeline` crate instead of `pub`, so the
compiler now rejects a `web`-crate import that reaches past the intended
API. This is the actual boundary-enforcement mechanism; it doesn't currently
exist (everything is `pub` today), which is the concrete gap this design
closes.

### Tests

Integration tests under `apps/web/tests/*.rs` exercise the assembled app
(`AppState`, `app()`), so they move to `apps/web/web/tests/`. `cargo test`
run from the workspace root (`apps/web/`) continues to run every crate's
tests, including `web`'s integration tests — no change needed in
`ci-web.yml`, which already runs `cargo test` with `working-directory:
apps/web`.

### `sqlx` offline mode

`sqlx::query!`/`query_as!` macros in `domain`, `pipeline`, and `web` resolve
against the `.sqlx` cache or `DATABASE_URL` at compile time. Cargo workspaces
build from the workspace root, so the existing `apps/web/.sqlx/` directory
is reachable by all three crates without changes, as long as
`SQLX_OFFLINE=true` (or `DATABASE_URL`) is set the same way it is today.

### CI

No changes required to `.github/workflows/ci-web.yml`: it already sets
`working-directory: apps/web` and runs `cargo fmt --check`, `cargo clippy
--all-targets --all-features`, `cargo test`, `cargo build --release` — all
of which operate on the whole workspace when run from the workspace root.

### Documentation

`apps/web/CLAUDE.md` gets updated to describe the three-crate layout,
per-crate ownership (`domain` = business rules, `pipeline` = document
ingestion pipeline, `web` = HTTP layer + jobs + middleware), and the
dependency direction (`web` → `{domain, pipeline}`, `domain` and `pipeline`
never depend on each other).

## Out of scope

- No change to runtime behavior, routes, database schema, or job scheduling.
- No split of `pipeline` into further sub-crates (e.g. per-stage crates for
  `extractor`/`parser`/`resolver`) — the sub-stages are cohesive enough to
  stay as modules within one `pipeline` crate; only `domain`, `pipeline`,
  and `web` become separate crates.
- No change to how the binary is deployed (still one `shovelsup-web` binary,
  one process).
- Android and iOS apps are untouched.

## Risks

- Mechanical but wide-reaching: every `use crate::domain::...` /
  `use crate::pipeline::...` in the `web` crate becomes `use
  shovelsup_domain::...` / `use shovelsup_pipeline::...`; every intra-module
  `use crate::...` inside `domain` and `pipeline` stays as-is (still `crate::`
  within their own crate) but the crate root shifts down one directory
  level (`domain/src/lib.rs` instead of `src/domain/mod.rs`), so all
  relative module paths inside those trees need adjusting during the move,
  not just import statements at the boundary.
- Partitioning `[dependencies]` across three `Cargo.toml`s risks missing a
  transitive need (e.g. a dev-dependency like `wiremock` used only by a test
  that's moving into `web/tests/`) — needs a full `cargo build --all-targets
  --all-features` plus `cargo test` pass after the split, not just a
  `cargo check` on the touched crate.
