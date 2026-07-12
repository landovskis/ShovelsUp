# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Stack

Rust server using **Axum** with **Minijinja** templates. `apps/web` is a Cargo workspace of three crates:

- `domain/` (`shovelsup-domain`) — business rules (`business_days`, `review_queue`). No dependency on `pipeline` or `web`.
- `pipeline/` (`shovelsup-pipeline`) — the document ingestion pipeline (`fetcher`, `parser`, `extractor`, `normalizer`, `resolver`, `redaction`, `scheduler`, `metrics`). No dependency on `domain` or `web`.
- `web/` (`shovelsup-web`) — the HTTP layer: routes (`web/src/routes/`), background jobs (`web/src/jobs/`), middleware (`web/src/middleware/`), feature flags (`web/src/config/`), and `main.rs`/`lib.rs` (`AppState`, `app()`). Depends on both `domain` and `pipeline`.

Dependencies only point toward `web`: `domain` and `pipeline` never depend on each other or on `web`. Within `pipeline`, most submodules are `pub(crate)` — only the items `web` or its tests actually call are `pub`. When adding a new pipeline submodule, default it to `pub(crate)` and widen only if something outside the crate needs it.

Templates live in `templates/` and static assets in `static/`, both at the workspace root (`apps/web/`), loaded by the `web` binary via paths relative to its working directory (`cargo run`/the deployed process's cwd), not relative to the crate.

`AppState` holds a shared `minijinja::Environment` passed via Axum's `.with_state()`.

## Commands

```bash
cargo run -p shovelsup-web       # dev server on :3000
cargo build --workspace
cargo test --workspace
RUST_LOG=debug cargo run -p shovelsup-web   # verbose logging
```

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan
<!-- SPECKIT END -->
