# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Product

ShovelsUp is a construction permit tracking app, currently focused on Montreal. The first major feature imports and parses Montreal City Council meeting agendas/minutes to surface construction and development decisions to residents and journalists.

## Repository Structure

Monorepo with three platform apps, no shared code between them yet:

```
apps/
  web/      — Rust/Axum web server
  android/  — Kotlin/Jetpack Compose
  ios/      — SwiftUI
```

Each app has its own `CLAUDE.md` with stack details and commands.
