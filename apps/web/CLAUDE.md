# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Stack

Rust server using **Axum** with **Minijinja** templates. Templates live in `templates/`, static assets in `static/`. Routes are in `src/routes/mod.rs`.

`AppState` holds a shared `minijinja::Environment` passed via Axum's `.with_state()`.

## Commands

```bash
cargo run                    # dev server on :3000
cargo build
cargo test
RUST_LOG=debug cargo run     # verbose logging
```
