# ADR 009 — Playwright (TypeScript) for Browser-Driven System Tests

**Status**: Accepted
**Date**: 2026-07-13
**Feature**: Implementation Plan: Public Discovery & Search (IMP-REQ-011-13)

## Context

REQ-011's mobile-responsive search experience has two test cases (TC-011-2, measuring
`document.documentElement.scrollWidth`/`clientWidth` for horizontal-scroll regressions,
and TC-011-3, driving focus management on a mobile filter sheet) that need a real headless
browser — they cannot be expressed as static-HTML/HTTP assertions against `app()` via
`tower::ServiceExt::oneshot`, unlike the rest of REQ-011's and this crate's test suite.

The Implementation Plan's own Autonomous Execution Contract specified evaluating
`fantoccini` (a Rust WebDriver client) first, falling back to a Node/Playwright CI stage
only if it proved unreliable. That evaluation happened and `fantoccini` was not usable in
this environment:

- `fantoccini` requires a running `chromedriver` binary.
- The only obtainable `chromedriver` build in this environment
  (`brew install --cask chromedriver`) is unsigned/non-notarized and is rejected outright
  by macOS Gatekeeper (`spctl -a -vv` → "rejected"), with no interactive approval path
  available in a non-interactive session.
- Re-signing the binary or otherwise bypassing Gatekeeper was considered and correctly
  refused as an unauthorized security-control bypass, not an engineering trade-off this
  task is entitled to make unilaterally.

Options considered:

| Option | Description |
|--------|-------------|
| `fantoccini` + `chromedriver` | Rust-native WebDriver client; blocked by Gatekeeper rejecting the only obtainable `chromedriver` build |
| Playwright (TypeScript), standalone Node project | Manages its own signed/notarized browser binaries via `npx playwright install`, sidestepping Gatekeeper entirely |
| Skip TC-011-2/-3 entirely, ship without browser-driven coverage | Leaves two plan-mandated test cases permanently unimplemented |

## Decision

Adopt **Playwright (TypeScript)** as a standalone project at `apps/web/e2e/`, exactly the
fallback the plan names for this situation. Playwright manages its own
signed/notarized browser binaries (`npx playwright install chromium`), which sidesteps the
Gatekeeper problem entirely rather than working around it.

- `apps/web/e2e/playwright.config.ts` configures a single `chromium` project, `baseURL`
  defaulting to `http://localhost:3000` (overridable via `SHOVELSUP_BASE_URL`), and
  `trace: 'retain-on-failure'`.
- The harness expects `shovelsup-web` to **already be running** and reachable at
  `baseURL` before `npm test` runs (e.g. `cargo run -p shovelsup-web &` from `apps/web/`,
  waited on until ready). It deliberately does not use Playwright's `webServer` option to
  auto-start the Rust server, since doing so reliably requires the Postgres/Redis dev
  stack and migrations to already be in a known-good state — provisioning and verifying
  that is outside what this harness can do or verify itself.
- TC-011-2 lives in `apps/web/e2e/tests/no-horizontal-scroll.spec.ts`, TC-011-3 in
  `apps/web/e2e/tests/filter-sheet.spec.ts`; the remaining REQ-011
  integration/accessibility coverage (IMP-REQ-011-14..17) lives in the same project. No
  Rust-side placeholder or `#[ignore]`d stub is kept for these two cases in
  `tests/responsive_e2e.rs` — the real assertions exist and pass in the Playwright suite,
  so a Rust stub would be dead, misleading weight.
- This is a genuinely new project dependency: a Node.js/npm toolchain now exists in the
  repo alongside the existing all-Rust (Cargo) stack. That is exactly why this decision
  gets an ADR, per the "a dependency/framework is introduced" trigger — every other test
  in this codebase runs under `cargo nextest`.

## Rationale

- Playwright's self-managed browser binaries are the direct fix for the actual blocker
  (Gatekeeper rejecting an unsigned `chromedriver`), not a workaround that defers the
  problem.
- Skipping TC-011-2/-3 was rejected: both are plan-mandated test cases with no
  browser-free equivalent, and the plan's own contract already names Playwright as the
  correct fallback for exactly this situation.
- No other Rust-native WebDriver client was evaluated as an alternative to `fantoccini`,
  since the blocker (no usable local `chromedriver`) applies to any WebDriver-protocol
  client, not specifically to `fantoccini`'s API.

## Consequences

- **A second toolchain (Node/npm) now exists in this repository alongside Cargo.**
  Contributors working on REQ-011-adjacent features need both toolchains installed, and
  `apps/web/e2e/` has its own `package.json`/lockfile independent of the Cargo workspace.
- **CI will need both toolchains available**, plus a step to start `shovelsup-web` (with
  a migrated Postgres/Redis available to it) before invoking `npx playwright test`, since
  this harness does not provision or start the server itself.
- **Precedent for future browser-driven test cases.** Any future requirement needing
  real-browser behavior (layout measurement, focus/keyboard-navigation assertions,
  visual regression) should extend `apps/web/e2e/` rather than re-evaluating
  `fantoccini`/`chromedriver` from scratch, unless the Gatekeeper/signing constraint that
  motivated this decision no longer applies in the target environment.
