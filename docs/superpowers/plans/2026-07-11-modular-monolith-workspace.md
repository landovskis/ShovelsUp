# Modular Monolith Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the single `shovelsup-web` crate at `apps/web` into a three-crate Cargo workspace (`domain`, `pipeline`, `web`) with compiler-enforced module boundaries, with zero behavior change.

**Architecture:** `apps/web/Cargo.toml` becomes a workspace-only manifest. Three member crates live in `apps/web/{domain,pipeline,web}/`. `domain` and `pipeline` are independent siblings (verified: neither imports the other); `web` depends on both and owns everything HTTP-facing (routes, jobs, middleware, config) plus the binary entrypoint. Internal-only pipeline submodules become `pub(crate)` so the compiler rejects cross-boundary reach-ins that are currently possible only because everything is `pub`.

**Tech Stack:** Rust 2021, Cargo workspaces, Axum, sqlx (Postgres, offline mode via `.sqlx/`), no new dependencies.

## Global Constraints

- No behavior change: routes, database schema, job scheduling, and HTTP responses must be identical before and after.
- `.github/workflows/ci-web.yml` must keep working unmodified (`working-directory: apps/web`, plain `cargo fmt/clippy/test/build` commands run from the workspace root).
- `apps/web/Dockerfile` must keep working unmodified (`COPY . .` into the build stage, `cargo build --release --bin shovelsup-web`).
- The binary package/bin name stays `shovelsup-web` and the library crate name for the web layer stays `shovelsup_web` (external tooling/scripts may reference these).
- `apps/web/.sqlx/` (offline query cache) stays at the workspace root — confirmed via sqlx docs that `cargo sqlx prepare --workspace` writes a single cache there and workspace builds resolve it from the workspace root.
- `sqlx::migrate!("./migrations")` resolves relative to the invoking crate's own `Cargo.toml` directory (confirmed via sqlx docs: "relative to the project root, the directory containing `Cargo.toml`"), so `migrations/` must move together with `main.rs` into the `web` crate to keep this macro call unchanged.
- Every task must end with `cargo build --workspace --all-targets --all-features` and `cargo test --workspace` both passing before moving to the next task.

---

### Task 1: Convert `apps/web` into a Cargo workspace with `web` as the sole member

This is a pure move: no module is extracted yet, no `use` statements change (the crate name `shovelsup_web` and its internal `crate::` paths are unaffected by relocating the whole crate one directory deeper). This isolates "did the workspace conversion mechanics work" from "did the domain/pipeline extraction work," so if something breaks here it's obviously the move, not an import fix.

**Files:**
- Move: `apps/web/src/` → `apps/web/web/src/`
- Move: `apps/web/Cargo.toml` → `apps/web/web/Cargo.toml`
- Move: `apps/web/migrations/` → `apps/web/web/migrations/`
- Move: `apps/web/tests/*.rs` (10 files, NOT `tests/fixtures/` or `tests/e2e/`) → `apps/web/web/tests/*.rs`
- Create: `apps/web/Cargo.toml` (new workspace manifest, replaces the moved one)
- Unchanged in place: `apps/web/templates/`, `apps/web/static/`, `apps/web/.sqlx/`, `apps/web/tests/fixtures/`, `apps/web/tests/e2e/`, `apps/web/docker-compose.yml`, `apps/web/Dockerfile`, `apps/web/.env*`, `apps/web/loadtest/`, `apps/web/data/`, `apps/web/Cargo.lock` (cargo updates it in place)

**Interfaces:**
- Produces: workspace member `web` (package `shovelsup-web`, lib `shovelsup_web`, bin `shovelsup-web`) building and testing exactly as before, just relocated.

- [ ] **Step 1: Move the crate into `web/`**

```bash
cd apps/web
mkdir -p web
git mv src web/src
git mv Cargo.toml web/Cargo.toml
git mv migrations web/migrations
mkdir -p web/tests
git mv tests/admin_routes.rs web/tests/admin_routes.rs
git mv tests/pipeline_extraction_fr.rs web/tests/pipeline_extraction_fr.rs
git mv tests/pipeline_extraction.rs web/tests/pipeline_extraction.rs
git mv tests/pipeline_fetch.rs web/tests/pipeline_fetch.rs
git mv tests/pipeline_resolver.rs web/tests/pipeline_resolver.rs
git mv tests/pipeline_scheduler.rs web/tests/pipeline_scheduler.rs
git mv tests/review_queue_e2e.rs web/tests/review_queue_e2e.rs
git mv tests/search_integration.rs web/tests/search_integration.rs
git mv tests/status_normalization_parity.rs web/tests/status_normalization_parity.rs
git mv tests/timeline_resolver.rs web/tests/timeline_resolver.rs
```

`tests/fixtures/` and `tests/e2e/` are untouched by this — they stay at `apps/web/tests/fixtures/` and `apps/web/tests/e2e/`. This matters because `apps/web/web/src/pipeline/parser/{pdf.rs,ocr.rs,orchestrate.rs}` load fixtures at compile time via `include_bytes!("../../../tests/fixtures/....")`, a path relative to the *source file's own location* (not `CARGO_MANIFEST_DIR`). The source files move from `src/pipeline/parser/` to `web/src/pipeline/parser/` — same depth (3 path segments below `apps/web/`) — so `../../../tests/fixtures/...` still resolves to `apps/web/tests/fixtures/...` without editing those `include_bytes!` calls.

- [ ] **Step 2: Write the new workspace root manifest**

Create `apps/web/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["web"]
```

- [ ] **Step 3: Build and test**

```bash
cd apps/web
cargo build --workspace --all-targets --all-features
cargo test --workspace
```

Expected: builds and all existing tests pass, identical to before the move (Cargo.lock will be rewritten in place to reflect the new workspace layout — that's expected).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(web): convert apps/web into a Cargo workspace with a single web member"
```

---

### Task 2: Extract the `domain` crate

`domain` (`business_days.rs`, `review_queue.rs`) has no dependency on `pipeline` and no internal cross-references between its two files — it's a clean, independent extraction.

**Files:**
- Move: `apps/web/web/src/domain/business_days.rs` → `apps/web/domain/src/business_days.rs`
- Move: `apps/web/web/src/domain/review_queue.rs` → `apps/web/domain/src/review_queue.rs`
- Move: `apps/web/web/src/domain/mod.rs` → `apps/web/domain/src/lib.rs`
- Create: `apps/web/domain/Cargo.toml`
- Modify: `apps/web/Cargo.toml` (add `domain` to members)
- Modify: `apps/web/web/Cargo.toml` (add `shovelsup-domain` path dependency)
- Modify: `apps/web/web/src/lib.rs` (remove `pub mod domain;`)
- Modify: `apps/web/web/src/routes/review_queue.rs:20` (fix import)

**Interfaces:**
- Produces: crate `shovelsup_domain` exposing `pub mod business_days;` (fn `add_business_days(start: DateTime<Utc>, days: i64) -> DateTime<Utc>`) and `pub mod review_queue;` (fns `confirm_candidate`, `reject_candidate`, type `ReviewQueueError`, both taking `&PgPool` — signatures unchanged from today, only the crate path changes).

- [ ] **Step 1: Move domain into its own crate**

```bash
cd apps/web
mkdir -p domain/src
git mv web/src/domain/business_days.rs domain/src/business_days.rs
git mv web/src/domain/review_queue.rs domain/src/review_queue.rs
git mv web/src/domain/mod.rs domain/src/lib.rs
```

- [ ] **Step 2: Create `apps/web/domain/Cargo.toml`**

```toml
[package]
name = "shovelsup-domain"
version = "0.1.0"
edition = "2021"

[lib]
name = "shovelsup_domain"
path = "src/lib.rs"

[dependencies]
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "chrono", "uuid"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
thiserror = "1"
```

- [ ] **Step 3: Add `domain` to the workspace**

Edit `apps/web/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["domain", "web"]
```

- [ ] **Step 4: Point `web` at the new crate**

Edit `apps/web/web/Cargo.toml`, add to `[dependencies]`:

```toml
shovelsup-domain = { path = "../domain" }
```

- [ ] **Step 5: Remove the now-empty module declaration**

Edit `apps/web/web/src/lib.rs`, delete this line:

```rust
pub mod domain;
```

- [ ] **Step 6: Fix the one caller**

Edit `apps/web/web/src/routes/review_queue.rs`, change line 20 from:

```rust
use crate::domain::review_queue::{confirm_candidate, reject_candidate, ReviewQueueError};
```

to:

```rust
use shovelsup_domain::review_queue::{confirm_candidate, reject_candidate, ReviewQueueError};
```

- [ ] **Step 7: Build and test**

```bash
cd apps/web
cargo build --workspace --all-targets --all-features
cargo test --workspace
```

Expected: builds clean, all tests pass (domain's own `#[cfg(test)]` unit tests in `business_days.rs` and `review_queue.rs` now run as part of the `shovelsup-domain` crate's test suite).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(web): extract shovelsup-domain crate from apps/web"
```

---

### Task 3: Extract the `pipeline` crate

`pipeline` has no dependency on `domain`. Its only internal cross-reference (`extractor` → `normalizer`/`redaction`) stays inside the new crate, just written as `crate::` instead of `crate::pipeline::` once `pipeline` becomes its own crate root. Six of the ten integration test files under `web/tests/` actually exercise `pipeline` directly (they import `shovelsup_web::pipeline::...`, never `AppState`/`app()`), so they move to `pipeline/tests/` in this task and their imports are rewritten to `shovelsup_pipeline::...`. The other four (`admin_routes.rs`, `review_queue_e2e.rs`, `search_integration.rs`, `timeline_resolver.rs`) exercise the assembled app and stay in `web/tests/`; two of those (`review_queue_e2e.rs`, `timeline_resolver.rs`) also import `pipeline::resolver::resolve_mention` and need that one import line fixed even though the file itself doesn't move.

**Files:**
- Move: `apps/web/web/src/pipeline/` (all files) → `apps/web/pipeline/src/`
- Move: `apps/web/web/src/pipeline/mod.rs` → `apps/web/pipeline/src/lib.rs`
- Create: `apps/web/pipeline/Cargo.toml`
- Modify: `apps/web/Cargo.toml` (add `pipeline` to members)
- Modify: `apps/web/web/Cargo.toml` (add `shovelsup-pipeline` path dependency; remove now-unused pipeline-only deps: `reqwest`, `scraper`, `whatlang`, `encoding_rs`, `sha2`, `async-trait`)
- Modify: `apps/web/web/src/lib.rs` (remove `pub mod pipeline;`)
- Modify: `apps/web/web/src/routes/admin.rs:9` (fix import)
- Modify: `apps/web/pipeline/src/extractor/mod.rs:9,204` (fix internal `crate::pipeline::` references)
- Move + modify: 6 test files from `apps/web/web/tests/` → `apps/web/pipeline/tests/`
- Modify: `apps/web/web/tests/review_queue_e2e.rs`, `apps/web/web/tests/timeline_resolver.rs` (fix one import line each, files stay in place)

**Interfaces:**
- Produces: crate `shovelsup_pipeline` exposing `pub mod {extractor, fetcher, metrics, normalizer, parser, redaction, resolver, scheduler};` — same module tree and same public items as today, just under a new crate root. Notable public items other crates/tests rely on: `extractor::{extract_entities, extract_and_store, llm::{LlmProvider, LlmError, AnthropicProvider}, schema::ExtractionResult}`, `fetcher::{Fetcher, FetchError, FetchOutcome}`, `normalizer::normalize_status`, `parser::{parse_document, ParseError, ParseMethod, ParsedChunk, ParseOutcome, ocr::TesseractOcrProvider, orchestrate::parse_and_store}`, `resolver::{resolve_mention, ResolutionOutcome, ResolveError}`, `scheduler::Scheduler`.

- [ ] **Step 1: Move pipeline into its own crate**

```bash
cd apps/web
mkdir -p pipeline/src
git mv web/src/pipeline/extractor pipeline/src/extractor
git mv web/src/pipeline/fetcher.rs pipeline/src/fetcher.rs
git mv web/src/pipeline/metrics.rs pipeline/src/metrics.rs
git mv web/src/pipeline/normalizer pipeline/src/normalizer
git mv web/src/pipeline/parser pipeline/src/parser
git mv web/src/pipeline/redaction pipeline/src/redaction
git mv web/src/pipeline/resolver pipeline/src/resolver
git mv web/src/pipeline/scheduler.rs pipeline/src/scheduler.rs
git mv web/src/pipeline/mod.rs pipeline/src/lib.rs
rmdir web/src/pipeline
```

- [ ] **Step 2: Create `apps/web/pipeline/Cargo.toml`**

```toml
[package]
name = "shovelsup-pipeline"
version = "0.1.0"
edition = "2021"

[lib]
name = "shovelsup_pipeline"
path = "src/lib.rs"

[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "chrono", "uuid"] }
reqwest = { version = "0.12", features = ["json"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
async-trait = "0.1"
scraper = "0.20"
whatlang = "0.16"
thiserror = "1"
sha2 = "0.10"
encoding_rs = "0.8"

[dev-dependencies]
wiremock = "0.6"
tokio = { version = "1", features = ["test-util"] }
```

- [ ] **Step 3: Add `pipeline` to the workspace**

Edit `apps/web/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["domain", "pipeline", "web"]
```

- [ ] **Step 4: Point `web` at the new crate and drop now-unused deps**

Edit `apps/web/web/Cargo.toml`. Add to `[dependencies]`:

```toml
shovelsup-pipeline = { path = "../pipeline" }
```

Remove these lines from `apps/web/web/Cargo.toml`'s `[dependencies]` (nothing left in `web/src` uses them once `pipeline` is extracted — confirmed by grep, only `pipeline/` code used them):

```
reqwest = { version = "0.12", features = ["json"] }
async-trait = "0.1"
scraper = "0.20"
whatlang = "0.16"
sha2 = "0.10"
encoding_rs = "0.8"
```

- [ ] **Step 5: Remove the now-empty module declaration**

Edit `apps/web/web/src/lib.rs`, delete this line:

```rust
pub mod pipeline;
```

- [ ] **Step 6: Fix the caller in `routes/admin.rs`**

Edit `apps/web/web/src/routes/admin.rs`, change line 9 from:

```rust
use crate::pipeline::parser::{ocr::TesseractOcrProvider, orchestrate::parse_and_store};
```

to:

```rust
use shovelsup_pipeline::parser::{ocr::TesseractOcrProvider, orchestrate::parse_and_store};
```

- [ ] **Step 7: Fix the internal reference inside the new crate**

Edit `apps/web/pipeline/src/extractor/mod.rs`, change line 9 from:

```rust
use crate::pipeline::{normalizer, redaction};
```

to:

```rust
use crate::{normalizer, redaction};
```

And change line 204 from:

```rust
if let Err(err) = crate::pipeline::resolver::resolve_mention(pool, mention_id).await {
```

to:

```rust
if let Err(err) = crate::resolver::resolve_mention(pool, mention_id).await {
```

- [ ] **Step 8: Move the pipeline-only integration tests and fix their imports**

```bash
cd apps/web
mkdir -p pipeline/tests
git mv web/tests/pipeline_extraction_fr.rs pipeline/tests/pipeline_extraction_fr.rs
git mv web/tests/pipeline_extraction.rs pipeline/tests/pipeline_extraction.rs
git mv web/tests/pipeline_fetch.rs pipeline/tests/pipeline_fetch.rs
git mv web/tests/pipeline_resolver.rs pipeline/tests/pipeline_resolver.rs
git mv web/tests/pipeline_scheduler.rs pipeline/tests/pipeline_scheduler.rs
git mv web/tests/status_normalization_parity.rs pipeline/tests/status_normalization_parity.rs
```

In each moved file, replace every `shovelsup_web::pipeline::` with `shovelsup_pipeline::`. Concretely:

`pipeline/tests/pipeline_extraction_fr.rs` lines 31–32, from:
```rust
use shovelsup_web::pipeline::extractor::extract_entities;
use shovelsup_web::pipeline::extractor::llm::AnthropicProvider;
```
to:
```rust
use shovelsup_pipeline::extractor::extract_entities;
use shovelsup_pipeline::extractor::llm::AnthropicProvider;
```
And line 103, from `&shovelsup_web::pipeline::extractor::schema::ExtractionResult` to `&shovelsup_pipeline::extractor::schema::ExtractionResult`.

`pipeline/tests/pipeline_extraction.rs` lines 37–38 (same substitution as above), and line 132 (same `schema::ExtractionResult` substitution as above).

`pipeline/tests/pipeline_fetch.rs` line 1, from:
```rust
use shovelsup_web::pipeline::fetcher::{FetchError, FetchOutcome, Fetcher};
```
to:
```rust
use shovelsup_pipeline::fetcher::{FetchError, FetchOutcome, Fetcher};
```

`pipeline/tests/pipeline_resolver.rs` lines 1–2, from:
```rust
use shovelsup_web::pipeline::resolver::resolve_mention;
use shovelsup_web::pipeline::resolver::ResolutionOutcome;
```
to:
```rust
use shovelsup_pipeline::resolver::resolve_mention;
use shovelsup_pipeline::resolver::ResolutionOutcome;
```

`pipeline/tests/pipeline_scheduler.rs` line 2, from:
```rust
use shovelsup_web::pipeline::scheduler::Scheduler;
```
to:
```rust
use shovelsup_pipeline::scheduler::Scheduler;
```

`pipeline/tests/status_normalization_parity.rs` line 6, from:
```rust
use shovelsup_web::pipeline::normalizer::normalize_status;
```
to:
```rust
use shovelsup_pipeline::normalizer::normalize_status;
```

- [ ] **Step 9: Fix the two app-level tests that stay in `web/tests/` but import pipeline items**

Edit `apps/web/web/tests/review_queue_e2e.rs` line 16, from:
```rust
use shovelsup_web::pipeline::resolver::resolve_mention;
```
to:
```rust
use shovelsup_pipeline::resolver::resolve_mention;
```
Also fix the fully-qualified reference at line 397, from:
```rust
shovelsup_web::pipeline::resolver::ResolutionOutcome::NewProject { project_id } => project_id,
```
to:
```rust
shovelsup_pipeline::resolver::ResolutionOutcome::NewProject { project_id } => project_id,
```

Edit `apps/web/web/tests/timeline_resolver.rs` line 28, from:
```rust
use shovelsup_web::pipeline::resolver::resolve_mention;
```
to:
```rust
use shovelsup_pipeline::resolver::resolve_mention;
```
Also fix the fully-qualified reference at line 107, from:
```rust
shovelsup_web::pipeline::resolver::ResolutionOutcome::FlaggedAmbiguous { review_candidate_id } => {
```
to:
```rust
shovelsup_pipeline::resolver::ResolutionOutcome::FlaggedAmbiguous { review_candidate_id } => {
```

`shovelsup_pipeline` is reachable from `web/tests/*.rs` because it's a `[dependencies]` entry of `web/Cargo.toml` (added in Step 4) — Cargo makes a package's own dependencies available to that package's integration tests automatically, no separate `[dev-dependencies]` entry needed.

- [ ] **Step 10: Build and test**

```bash
cd apps/web
cargo build --workspace --all-targets --all-features
cargo test --workspace
```

Expected: builds clean, all tests pass, including `pipeline`'s own `#[cfg(test)]` unit tests (e.g. in `parser/pdf.rs`, `parser/ocr.rs`, `parser/orchestrate.rs`, `parser/mod.rs`, `redaction/mod.rs`, `extractor/mod.rs`, `extractor/llm.rs`) now running as part of the `shovelsup-pipeline` crate's test suite, and the 6 relocated integration test files now compiling against `shovelsup_pipeline` instead of `shovelsup_web::pipeline`.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(web): extract shovelsup-pipeline crate from apps/web"
```

---

### Task 4: Enforce module boundaries inside `shovelsup-pipeline`

Today every pipeline submodule is `pub`, meaning nothing stops code elsewhere in the workspace from reaching into implementation details like `extractor::validator` or `resolver::address`. This task narrows visibility to `pub(crate)` for submodules that are pure internals — verified by grepping the whole workspace (`src/`, `tests/`, both before and after the moves in Tasks 1-3) for any usage from outside `pipeline` itself. Submodules whose items are used by `web` or by the integration tests in `pipeline/tests/`/`web/tests/` stay `pub`.

**Files:**
- Modify: `apps/web/pipeline/src/extractor/mod.rs`
- Modify: `apps/web/pipeline/src/parser/mod.rs`
- Modify: `apps/web/pipeline/src/redaction/mod.rs`
- Modify: `apps/web/pipeline/src/resolver/mod.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `shovelsup_pipeline`'s public surface shrinks to exactly the items verified as used externally (listed in Task 3's Interfaces block); everything else becomes crate-internal, so the compiler now rejects a `web`-crate or test-crate import that reaches past it.

- [ ] **Step 1: Narrow `extractor` submodules**

Edit `apps/web/pipeline/src/extractor/mod.rs`. `schema` and `llm` stay `pub` (used by `pipeline/tests/`). `validator`, `scale`, and `prompts` have zero external references — change:

```rust
pub mod llm;
pub mod prompts;
pub mod schema;
pub mod scale;
pub mod validator;
```

to:

```rust
pub mod llm;
pub(crate) mod prompts;
pub mod schema;
pub(crate) mod scale;
pub(crate) mod validator;
```

- [ ] **Step 2: Narrow `parser` submodules**

Edit `apps/web/pipeline/src/parser/mod.rs`. `ocr` and `orchestrate` stay `pub` (used by `routes/admin.rs` via `shovelsup_pipeline::parser::{ocr::TesseractOcrProvider, orchestrate::parse_and_store}`). `html`, `lang`, `pdf`, `plaintext` have zero external references — change:

```rust
pub mod html;
pub mod lang;
pub mod ocr;
pub mod orchestrate;
pub mod plaintext;
pub mod pdf;
```

to:

```rust
pub(crate) mod html;
pub(crate) mod lang;
pub mod ocr;
pub mod orchestrate;
pub(crate) mod plaintext;
pub(crate) mod pdf;
```

- [ ] **Step 3: Narrow `redaction` submodule**

Edit `apps/web/pipeline/src/redaction/mod.rs`. `fr` is only used from `extractor/mod.rs`, inside the same crate — change:

```rust
pub mod fr;
```

to:

```rust
pub(crate) mod fr;
```

- [ ] **Step 4: Narrow `resolver` submodules**

Edit `apps/web/pipeline/src/resolver/mod.rs`. `address` and `address_fr` have zero external references — change:

```rust
pub mod address;
pub mod address_fr;
```

to:

```rust
pub(crate) mod address;
pub(crate) mod address_fr;
```

- [ ] **Step 5: Build and test**

```bash
cd apps/web
cargo build --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Expected: builds clean with no warnings and all tests pass. If `cargo build` reports a private-module error, it means one of the four verification greps above missed a caller — revert that specific submodule back to `pub` rather than guessing at a different fix, since the whole point of this task is to match visibility to actual, verified usage.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(pipeline): narrow submodule visibility to enforce crate-internal boundaries"
```

---

### Task 5: Update `apps/web/CLAUDE.md`

**Files:**
- Modify: `apps/web/CLAUDE.md`

**Interfaces:** none (documentation only).

- [ ] **Step 1: Rewrite the Stack section to describe the workspace**

Edit `apps/web/CLAUDE.md`, replace the `## Stack` section with:

```markdown
## Stack

Rust server using **Axum** with **Minijinja** templates. `apps/web` is a Cargo workspace of three crates:

- `domain/` (`shovelsup-domain`) — business rules (`business_days`, `review_queue`). No dependency on `pipeline` or `web`.
- `pipeline/` (`shovelsup-pipeline`) — the document ingestion pipeline (`fetcher`, `parser`, `extractor`, `normalizer`, `resolver`, `redaction`, `scheduler`, `metrics`). No dependency on `domain` or `web`.
- `web/` (`shovelsup-web`) — the HTTP layer: routes (`web/src/routes/`), background jobs (`web/src/jobs/`), middleware (`web/src/middleware/`), feature flags (`web/src/config/`), and `main.rs`/`lib.rs` (`AppState`, `app()`). Depends on both `domain` and `pipeline`.

Dependencies only point toward `web`: `domain` and `pipeline` never depend on each other or on `web`. Within `pipeline`, most submodules are `pub(crate)` — only the items `web` or its tests actually call are `pub`. When adding a new pipeline submodule, default it to `pub(crate)` and widen only if something outside the crate needs it.

Templates live in `templates/` and static assets in `static/`, both at the workspace root (`apps/web/`), loaded by the `web` binary via paths relative to its working directory (`cargo run`/the deployed process's cwd), not relative to the crate.

`AppState` holds a shared `minijinja::Environment` passed via Axum's `.with_state()`.
```

- [ ] **Step 2: Update the Commands section**

Edit `apps/web/CLAUDE.md`, replace the `## Commands` section:

```markdown
## Commands

```bash
cargo run -p shovelsup-web       # dev server on :3000
cargo build --workspace
cargo test --workspace
RUST_LOG=debug cargo run -p shovelsup-web   # verbose logging
```
```

- [ ] **Step 3: Commit**

```bash
git add apps/web/CLAUDE.md
git commit -m "docs(web): document the domain/pipeline/web workspace split"
```
