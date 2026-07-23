# ADR 010 — One-Hop `merged_into_id` for Duplicate/Superseded Projects

**Status**: Accepted
**Date**: 2026-07-13
**Feature**: Implementation Plan: Public Discovery & Search (IMP-REQ-013-01/-04/-05/-06)

## Context

Two rows in `projects` can sometimes represent the same real-world permit/development —
e.g. a document is re-ingested under slightly different extracted text, or the resolver
(REQ-005) initially created two separate projects that an operator later determines are
duplicates. REQ-013 gives every project a shareable, canonical detail URL
(`GET /projects/{id}`); once such a duplicate is identified, the superseded project's URL
needs to keep working for anyone who already shared or bookmarked it, rather than 404ing
or continuing to render its own now-stale page.

Options considered:

| Option | Description |
|--------|-------------|
| Nullable self-referencing FK (`merged_into_id`) + DB trigger enforcing one-hop | Data-level invariant enforced by Postgres itself, not trusted to application code |
| Nullable self-referencing FK, invariant enforced only in application code | Simpler schema, but nothing stops a direct SQL write (a manual fix, a future migration, a different code path) from creating a multi-hop or cyclic chain |
| Separate `project_merges` join/history table | Supports richer history (multiple past merges, undo) but is more schema than REQ-013 asked for |
| Hard-delete the superseded project, keep only a redirect table keyed by old id | Loses the superseded project's own data entirely, not just its standalone page |

## Decision

Add a nullable, self-referencing column: `projects.merged_into_id UUID REFERENCES
projects(id) ON DELETE SET NULL` (migration `025_project_merged_into_id.sql`).

- **Nullable** because most projects are never merged.
- **`ON DELETE SET NULL`, not `CASCADE`/`RESTRICT`**: if the canonical target project is
  ever deleted, the merged project falls back to rendering its own page again rather than
  either cascading into an unrelated delete or blocking the target's deletion entirely.
- **One-hop invariant enforced by a `BEFORE INSERT OR UPDATE OF merged_into_id` trigger**
  (`enforce_projects_merged_into_one_hop`), not a plain column `CHECK` constraint. A
  `CHECK` on a single row cannot see other rows' data, and the invariant — "a merge chain
  is never more than one hop deep" (A → B is fine; A → B → C is not) — requires looking up
  whether the target row is itself already merged elsewhere, and whether the current row
  is already someone else's merge target. The trigger performs both the forward check
  (target is not itself merged) and the reverse check (this row is not already another
  row's target) before allowing the write, and self-references (`merged_into_id = id`) are
  rejected outright.
- **`GET /projects/:id`** (`routes/projects.rs`) queries `merged_into_id` alongside
  `id`/`confidence_level` before any other lookup, and issues a real
  `StatusCode::MOVED_PERMANENTLY` (301) redirect to `/projects/{merged_into_id}` the
  moment it is `Some` — never constructing the page's `ProjectDetailContext` or rendering
  the superseded project's own page at all. This is built as a literal 301 response
  (`StatusCode::MOVED_PERMANENTLY.into_response()`), not axum's `Redirect::permanent`
  helper, which emits a 308 (`PERMANENT_REDIRECT`) — semantically similar but a different
  status code than what the test suite and this decision settled on.

## Rationale

- Enforcing the one-hop invariant at the database level (via a trigger) rather than only
  in application code means the guarantee holds regardless of which code path writes to
  `merged_into_id` — a future admin tool, a manual data fix, or a different requirement's
  code cannot silently violate it.
- A `CHECK` constraint was rejected outright since Postgres `CHECK` constraints cannot
  reference other rows; only a trigger (or a deferred constraint trigger) can express a
  cross-row invariant like this one.
- A separate `project_merges` history table was rejected as disproportionate: REQ-013
  only requires that a shareable URL keep resolving somewhere sensible, not a full
  merge/undo history.
- A real 301 (not a 404, and not silently re-rendering stale content) is the correct HTTP
  semantic for "this resource has permanently moved" and lets search engines and browsers
  update their own records accordingly.

## Consequences

- **Merge chains are capped at one hop by construction.** If project B (merged into
  project C) needs to also be marked as merged into a different project D, the existing
  row must first be updated/cleared rather than layering a second hop on top — the
  trigger will reject any write that would create A → B → C.
- **Deleting a merge target un-merges its source project.** `ON DELETE SET NULL` means
  deleting project C automatically clears `merged_into_id` on any project that pointed to
  it, silently restoring that project's own standalone page. This is a deliberate
  trade-off (favoring "page still renders something" over "page always 404s or redirects
  correctly") that should be revisited if project deletion becomes a real, sanctioned
  operation.
- **No merge history is retained.** Once `merged_into_id` is cleared or repointed, there
  is no record in this schema of what it previously pointed to; `audit_events` (REQ-009's
  table) is not currently wired to record merge/unmerge actions.
