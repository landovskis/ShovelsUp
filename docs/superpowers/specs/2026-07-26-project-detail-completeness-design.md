# Design: Project Detail Page Shows All Project Information

**Date**: 2026-07-26
**Related**: `apps/web/web/src/routes/projects.rs` (`get_project_detail_page`), `apps/web/templates/project_detail.html`

## Problem

The project details page (`GET /projects/{id}`) currently renders only a subset of the
information the system already has about a project: a synthesized description,
confidence level, one citation (municipality + meeting date), a source-document link,
and a timeline. Several fields that exist in the database and are already fetched or
fetchable are never shown to the user:

- `project_name`, `reference_number` — selected today by
  `fetch_latest_mention_for_description` but discarded, never displayed.
- `civic_address_normalized`, `project_type`, `category_code` — columns on the
  `projects` table, never queried by the detail handler at all.
- `scale_units`, `scale_gfa_sqm`, `scale_storeys` — columns on `project_mentions`,
  used only as internal inputs to the synthesized description sentence, never shown
  as discrete fields.
- Category label (`category_taxonomy.label_en`/`label_fr`) — never joined on this
  page; category is currently used only by the search page's facets.

## Goal

Show all of the above as explicit, labeled fields on the project detail page, in
addition to what's already there. Replace the current scattered per-concern queries
(`project_row`, `fetch_latest_mention_for_description`'s field-selection role) with a
single consolidated `Project` struct and query, scoped to this handler only.

## Non-goals

- No shared/reusable `Project` model across other routes (search, timeline API) —
  those keep their own existing queries.
- No new Postgres view or migration — the consolidated query is plain SQL
  (`sqlx::query_as!` with JOINs) in Rust, matching this codebase's existing
  migration-light read-path approach.
- No change to the citation section's error-isolation behavior (see below).
- No change to timeline rendering — `fetch_timeline_events` is untouched.

## Data model: consolidated `Project` struct

Replaces `project_row`'s ad-hoc query and the field-selection half of
`fetch_latest_mention_for_description` (that function's description-synthesis role
either takes these fields as input, or its synthesis logic moves inline using the new
struct — implementer's choice, whichever is less code).

```rust
struct Project {
    id: Uuid,
    project_name: Option<String>,
    civic_address: Option<String>,
    project_type: Option<String>,
    category_code: Option<String>,
    category_label: Option<String>,
    reference_number: Option<String>,
    scale_units: Option<i32>,
    scale_gfa_sqm: Option<f64>,
    scale_storeys: Option<i32>,
    confidence_level: Option<String>,
    merged_into_id: Option<Uuid>,
    first_detected_at: Option<DateTime<Utc>>,
    source_count: Option<i64>,
    description_lang: Option<String>,
    source_document_url: Option<String>,
}
```

One `sqlx::query_as!` fetches all of the above in a single row per project:

- `FROM projects p`
- `LEFT JOIN category_taxonomy ct ON ct.code = p.category_code AND ct.is_public` —
  the `is_public` condition is part of the join, not a post-filter, so a non-public
  category silently yields `category_label = NULL` (category hidden) while
  `category_code` may still be present internally.
- Category label is locale-aware, chosen in SQL: `CASE WHEN $locale = 'fr' THEN
  ct.label_fr ELSE ct.label_en END`.
- `LEFT JOIN LATERAL (SELECT project_name, civic_address, project_type,
  reference_number, scale_units, scale_gfa_sqm, scale_storeys, approval_status_raw,
  language FROM project_mentions WHERE project_id = p.id ORDER BY created_at DESC
  LIMIT 1) m ON true` — "latest mention" wins on conflicting values across mentions,
  per product decision (simplest, consistent with the existing
  `fetch_latest_mention_for_description` pattern).
- `p.civic_address_normalized` / `p.project_type` (the canonical `projects` row)
  fill gaps when the latest mention's value is null — mention data is primary, the
  canonical row is a fallback only.
- `LEFT JOIN public_search_documents psd ON …` (same join key as today's
  `project_row` query) for `first_detected_at`, `source_count`.

**Citation stays separate.** `fetch_primary_citation` (municipality name, meeting
date, source reliability) is *not* folded into the wide query. It keeps its own
query call wrapped in `.ok()`, exactly as today, so a transient DB error there still
just hides the citation section instead of 503ing the whole page. Folding it in
would have removed that isolation for no benefit.

## Error handling

Unchanged shape: the wide query 503s on DB error (via `render_error_page`) and 404s
on a missing row, exactly like today's `project_row` query. No new failure modes.

## Template changes (`templates/project_detail.html`)

New "Project details" section, placed after the `<h1>`/detection-sentence area and
before the description paragraph. Each field is its own labeled row:

- Project name (existing `page_title`/`project.title` fallback logic is unaffected)
- Civic address
- Project type
- Category (localized label; present only when `is_public`, per the join above)
- Reference number
- Scale: units, GFA (m²), storeys — each its own row

**Null handling**: any field that is null is omitted entirely (no label rendered),
consistent with how the confidence-level and citation sections already degrade
gracefully. Fields are independent — e.g. GFA can show while storeys is omitted.

All labels are localized (EN/FR) through the existing Minijinja `context!`
locale mechanism used elsewhere on this page.

## Testing

Extend the detail-page integration test coverage (`apps/web/web/tests/` — sibling to
`search_integration.rs`, or a new file) with cases:

1. All new fields present → all rows render with correct values.
2. All new fields null → all rows omitted, no empty labels, rest of page unaffected.
3. Category with `is_public = true` → category row renders with correct locale label.
4. Category with `is_public = false` → category row is absent even though
   `category_code` is set on the project.
5. Existing tests (description synthesis, confidence level, citation, timeline,
   merged-project redirect) continue to pass unmodified — this change is additive.

If the locale-based category label `CASE` logic proves fiddly to verify through
integration tests alone, add a focused unit test for just that SQL fragment/helper.
