<!--
SYNC IMPACT REPORT
==================
Version change: 1.2.0 → 1.3.0
Type: MINOR — new Automated Testing principle added

Modified principles: none

Added sections:
  - Core Principles: VI. Living Documentation (C4 diagrams + ERD)
  - Development Workflow: Architecture Decision Records (ADR) sub-section (v1.1.0)

Removed sections: none

Templates checked:
  ✅ .specify/templates/plan-template.md — Constitution Check gate is dynamic; no changes needed
  ✅ .specify/templates/spec-template.md — Generic structure; no changes needed
  ✅ .specify/templates/tasks-template.md — Generic structure; no changes needed
  ✅ .specify/templates/commands/ — Directory does not exist; no command files to update

Deferred TODOs: none
-->

# ShovelsUp Web Constitution

## Core Principles

### I. Bilingual-First (NON-NEGOTIABLE)

Every user-facing string MUST be available in both English (EN) and French (FR).
Montreal is an officially bilingual city; single-language releases are not acceptable.

- Language selection MUST be driven by the `Accept-Language` HTTP header on each route.
- Translated strings MUST be passed via the Minijinja template context — no hardcoded
  display text in templates.
- Any PR that adds user-visible text without both EN and FR translations MUST be blocked.

**Rationale**: The target audience (Montreal residents and journalists) operates in both
languages. A missing translation is a broken feature.

### II. Safety & Correctness (Rust-native)

Rust's type system is the primary correctness tool; it MUST be used, not bypassed.

- `unwrap()` and `expect()` are FORBIDDEN in production code paths (non-test modules).
  Use `?` propagation or explicit `match`/`if let` with meaningful error context.
- All fallible operations MUST return `Result`; silent failures via swallowed errors are
  a defect, not a workaround.
- `cargo build` MUST succeed with zero warnings before any branch is merged.
- `cargo test` MUST pass before any branch is merged.

**Rationale**: This is a data-ingestion app; correctness of permit data matters to the
people who act on it. Silent failures erode trust.

### III. Server-Side Simplicity

The rendering model is server-side: Axum routes, Minijinja templates, static CSS.
Client-side JavaScript complexity MUST be justified before introduction.

- Routes live in `src/routes/mod.rs`; new routes MUST follow established handler patterns.
- Templates live in `templates/`; static assets live in `static/`.
- `AppState` is the single shared-state boundary; new state fields MUST be added there,
  not via thread-locals, statics, or ambient globals.
- Adding a JS framework or build pipeline requires explicit team decision and documented
  rationale in the relevant spec.

**Rationale**: The team is small and the stack is deliberately lean. Complexity MUST earn
its place.

### IV. Design System Compliance

Visual implementation MUST use the design tokens defined in `DESIGN.md`; raw values
are not permitted in source files.

- Web colors MUST be referenced via CSS custom properties: `var(--color-primary)` and
  `var(--color-text)`. Hex literals (`#E84E0F`, etc.) MUST NOT appear in templates or
  stylesheets.
- Typography uses the native system font stack (`system-ui, -apple-system, sans-serif`);
  no custom typefaces are loaded.
- Icons and logos MUST originate from `logo.svg` / `icon.svg` at the repo root; do not
  inline modified copies or alternative color variants.

**Rationale**: Consistent brand identity across web, Android, and iOS requires a single
source of truth for visual tokens.

### V. Data Integrity for Civic Information

Permit and council-agenda data surfaced to users MUST be traceable to its source
and free of silent transformation errors.

- Every import pipeline MUST log the source document (URL or filename), import timestamp,
  and item count for each run.
- Parse errors MUST be surfaced as structured errors (logged and/or stored), never
  silently dropped.
- Derived or transformed fields MUST be distinguished from raw source fields in the data
  model.

**Rationale**: Residents and journalists act on this data. Undetected import failures
or silent data corruption have real civic consequences.

### VI. Automated Testing (NON-NEGOTIABLE)

All three layers of automated testing MUST be present and passing for any feature
that reaches `main`. No exceptions.

**Unit tests**
- MUST cover individual functions, data transformations, and pure logic in isolation.
- Live in inline `#[cfg(test)]` modules within the same source file as the code under test.
- MUST run via `cargo test` with no external dependencies (no network, no database).
- New logic without a corresponding unit test is a defect.

**Integration tests**
- MUST cover HTTP handler behaviour end-to-end at the Axum layer: routing, middleware,
  request parsing, response shape, and error paths.
- Live in `tests/` at the crate root (Rust integration test convention).
- MAY use an in-process test server or `reqwest`/`axum::test` utilities; external
  services MUST be replaced with fakes or stubs — not mocks that diverge from real
  contracts.
- Every new route and every significant handler change MUST have integration test coverage.

**End-to-end (E2E) tests**
- MUST verify critical user journeys through a real running instance (browser-level or
  HTTP-level depending on the journey type).
- E2E tooling MUST be decided via ADR before the first E2E suite is written.
- At minimum, the P1 user story for each feature MUST have an E2E test before the
  feature is considered complete.
- E2E tests MUST run in CI on every PR; flaky E2E tests MUST be fixed or deleted within
  one sprint — they MUST NOT be skipped indefinitely.

**General rules**
- `cargo test` MUST pass with zero failures before any PR is merged (gates CI).
- Test coverage MUST NOT regress: a PR that deletes tests without replacing them requires
  explicit justification in the PR description.
- Tests are first-class code: they MUST follow the same quality standards as production
  code (no magic literals, no copy-paste chains, meaningful assertions).

**Rationale**: ShovelsUp surfaces civic data that people act on. Regressions in import
parsing or display logic have real-world consequences. Testing is not optional overhead —
it is how correctness is verified at every layer.

### VII. Living Documentation

Architecture and data model documentation MUST be kept current as the system evolves;
stale diagrams are a defect, not a cosmetic issue.

- **C4 diagrams** MUST be maintained in `docs/architecture/` covering at minimum:
  - **Level 1 — System Context**: how ShovelsUp sits within its environment
    (users, external data sources, browsers).
  - **Level 2 — Container**: the major deployable units (Axum server, database,
    static assets, external data feeds).
  - Level 3 (Component) and Level 4 (Code) diagrams are optional but encouraged for
    complex subsystems.
  - Diagrams MUST be authored in a text-based format (e.g., C4-PlantUML or Structurizr
    DSL) stored alongside their rendered outputs; binary-only diagrams are not acceptable.
- **ERD** MUST be maintained in `docs/architecture/erd.md` (or equivalent) and updated
  whenever the data model changes. It MUST reflect the current schema, not an aspirational
  or historical one.
- Any PR that changes the data model, adds a container/service, or materially alters a
  system boundary MUST include corresponding diagram updates in the same PR.
- Documentation freshness is a PR review gate: reviewers MUST verify that diagrams
  reflect the post-merge state of the system.

**Rationale**: C4 and ERD diagrams are the fastest way for a new contributor (or a
returning one) to understand system boundaries and data flow. Outdated diagrams are
actively harmful — they mislead more than no diagram at all.

## Technology Constraints

- **Language**: Rust (stable channel); use the latest stable toolchain.
- **Web framework**: Axum; do not introduce a competing async runtime or HTTP framework.
- **Templating**: Minijinja; template logic MUST remain minimal — business logic belongs
  in Rust handlers, not templates.
- **Database**: TBD per feature spec; when introduced, migrations MUST be versioned and
  idempotent.
- **CI**: GitHub Actions pipelines MUST run `cargo build` and `cargo test` on every PR.
  Failing CI blocks merge.
- **Dependencies**: Prefer the latest stable version of each crate. Pinning to an older
  version requires a documented reason in `Cargo.toml` (inline comment).

## Development Workflow

- All work MUST happen on feature branches; direct commits to `main` are not permitted.
- Branch naming convention: `feature/<short-description>` or `fix/<short-description>`.
- Every PR MUST include EN and FR coverage for any new user-facing text.
- The Constitution Check in each plan's `plan.md` MUST be completed before Phase 0
  research begins and re-validated after Phase 1 design.
- Complexity introduced in violation of a principle MUST be documented in the
  `Complexity Tracking` table of the relevant `plan.md`, with rationale.

### Architecture Decision Records (ADRs)

Significant technical decisions MUST be documented as ADRs before implementation begins.

- ADRs live in `docs/adr/` at the repository root, named `NNN-short-title.md`
  (e.g., `001-use-axum-for-http-layer.md`).
- An ADR is required when a decision meets any of these criteria:
  - Introduces or replaces a dependency, framework, or runtime.
  - Establishes a cross-cutting architectural pattern (data model structure, auth strategy,
    caching approach, async/sync boundary, etc.).
  - Deviates from an existing constitution principle (also requires Complexity Tracking entry).
  - Has non-trivial reversal cost if later found to be wrong.
- Each ADR MUST contain at minimum: **Title**, **Status** (Proposed / Accepted / Superseded),
  **Context** (why a decision is needed), **Decision** (what was decided),
  **Consequences** (trade-offs accepted).
- ADRs are append-only: superseded records MUST be retained and marked `Superseded by ADR-NNN`
  rather than deleted or edited.

## Governance

This constitution supersedes any conflicting guidance in `CLAUDE.md` or ad-hoc
conventions. `CLAUDE.md` captures tooling and command shortcuts; the constitution
captures non-negotiable engineering and product principles.

**Amendment procedure**:
1. Propose the amendment in a PR with updated `constitution.md`.
2. Bump the version according to semantic rules (see below).
3. Update dependent templates and `CLAUDE.md` references if affected.
4. Merge requires at least one explicit approval acknowledging the change.

**Versioning policy**:
- MAJOR: Principle removed, renamed, or its non-negotiable constraint materially relaxed.
- MINOR: New principle or section added; existing principle materially strengthened.
- PATCH: Clarifications, wording, typo fixes, examples added — no semantic change.

**Compliance review**: Constitution compliance MUST be checked at PR review time.
Reviewers are expected to flag violations; authors are responsible for addressing them
before merge.

**Version**: 1.3.0 | **Ratified**: 2026-06-25 | **Last Amended**: 2026-06-25
