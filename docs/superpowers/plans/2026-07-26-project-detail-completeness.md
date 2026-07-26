# Project Detail Page Completeness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `GET /projects/{id}` show all the project information the system already has (project name, civic address, project type, category, reference number, scale) instead of only a subset.

**Architecture:** Replace the two ad-hoc queries `project_row` (inline in the handler) and `fetch_latest_mention_for_description` with one consolidated `fetch_project`/`Project` struct query, scoped only to `get_project_detail_page`. `fetch_primary_citation` (citation section) and `fetch_timeline_events` (timeline section) are untouched — citation keeps its own `.ok()`-isolated query so a citation-side DB error still can't 503 the whole page. New fields are threaded through `ProjectDetailContext` into `project_detail.html`, each rendered as an independently-omittable labeled row.

**Tech Stack:** Rust, Axum, sqlx (`query_as!`), Minijinja, Postgres, `sqlx::test` integration tests (`cargo nextest run --workspace`).

## Global Constraints

- Bilingual EN/FR: every new label must have both an English and French string, following this file's existing `not_found_labels`/`cta_labels`/`timeline_labels` pattern (plain `match lang { "fr" => ..., _ => ... }`, no I/O).
- Functional core / imperative shell: any new pure decision logic (e.g. picking a locale's category label) goes in `mod core` if it doesn't already live as a plain shell computation; simple `if lang == "fr"` label picks mirror the existing `not_found_labels` style and don't need `core`.
- No new migration — the consolidated query is a plain `sqlx::query_as!` with JOINs against existing tables/columns.
- A field with a null/missing value must be omitted entirely from the rendered page (no label, no "Not available" placeholder), consistent with the rest of this page's graceful degradation.
- Category is shown only when its `category_taxonomy` row has `is_public = true`.
- Citation-section DB-error isolation (`.ok()` on `fetch_primary_citation`) must be preserved exactly as-is.

---

### Task 1: Failing test coverage for the new project-detail fields

**Files:**
- Modify: `apps/web/web/tests/timeline_resolver.rs`

**Interfaces:**
- Consumes: existing `seed_project(pool, address, project_type) -> Uuid`, `seed_document_chunk(pool) -> Uuid`, `seed_timeline_event(pool, project_id, mention_id, event_date, status) -> Uuid`, `test_state(pool) -> AppState`, `app(state)` (all already defined in this file).
- Produces: a new helper `insert_mention_with_details(pool, chunk_id, project_name, civic_address, project_type, reference_number, scale_units, scale_gfa_sqm, scale_storeys) -> Uuid`, used by Task 1's own tests and available for any later test in this file.

This task only adds tests and a fixture helper — it deliberately fails until Tasks 2 and 3 land, matching this file's existing convention (see the `tc_005_*`/`tc_015_*` "Currently FAILS" doc comments already in this file).

- [ ] **Step 1: Add the `insert_mention_with_details` fixture helper**

Add this function directly below the existing `insert_mention` function (after its closing brace, currently ending around line 153):

```rust
/// Same as `insert_mention`, but accepts every field the project-detail
/// page's new "Project details" section needs (project_name,
/// reference_number, scale_units/gfa/storeys), each independently
/// nullable so tests can exercise both "all present" and "all absent"
/// cases. `physical_work` is always `true` here (same as `insert_mention`),
/// so at least one of `scale_units`/`scale_gfa_sqm`/`scale_storeys` must be
/// `Some` to satisfy `project_mentions`' `scale_indicator_required_for_physical_work`
/// CHECK constraint.
#[allow(clippy::too_many_arguments)]
async fn insert_mention_with_details(
    pool: &PgPool,
    chunk_id: Uuid,
    project_name: Option<&str>,
    civic_address: Option<&str>,
    project_type: Option<&str>,
    reference_number: Option<&str>,
    scale_units: Option<i32>,
    scale_gfa_sqm: Option<f64>,
    scale_storeys: Option<i32>,
) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO project_mentions \
         (document_chunk_id, physical_work, project_name, civic_address, project_type, \
          reference_number, scale_units, scale_gfa_sqm, scale_storeys) \
         VALUES ($1, true, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        chunk_id,
        project_name,
        civic_address,
        project_type,
        reference_number,
        scale_units,
        scale_gfa_sqm,
        scale_storeys,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
```

- [ ] **Step 2: Add the "all fields present" test**

Append at the end of the file:

```rust
// ---------------------------------------------------------------------
// Project detail page completeness (docs/superpowers/specs/
// 2026-07-26-project-detail-completeness-design.md): the detail page must
// show project_name, civic_address, project_type, category (when public),
// reference_number, and scale (units/gfa/storeys) — fields that exist in
// the database but the page never rendered before this pass.
// ---------------------------------------------------------------------

/// A project whose most recent mention carries every new field, and whose
/// `category_code` points at a public taxonomy row, renders all of them as
/// labeled rows. The mention's own civic_address/project_type ("123 Main
/// St"/"residential") deliberately differ from the project's canonical
/// `civic_address_normalized`/`project_type` ("999 Canonical Fallback
/// Ave"/"institutional") so this test also pins down that the *mention's*
/// value wins when both are present (canonical is a fallback only).
#[sqlx::test(migrations = "./migrations")]
async fn project_detail_all_new_fields_render_when_present(pool: PgPool) {
    let project_id = seed_project(&pool, "999 Canonical Fallback Ave", "institutional").await;
    sqlx::query!(
        "UPDATE projects SET category_code = 'residential' WHERE id = $1",
        project_id
    )
    .execute(&pool)
    .await
    .unwrap();
    let chunk_id = seed_document_chunk(&pool).await;
    let mention_id = insert_mention_with_details(
        &pool,
        chunk_id,
        Some("Riverside Towers"),
        Some("123 Main St"),
        Some("residential"),
        Some("REF-2026-001"),
        Some(42),
        Some(1234.5),
        Some(12),
    )
    .await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains(r#"id="project-name""#) && html.contains("Riverside Towers"),
        "expected a #project-name row with the mention's project_name, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-civic-address""#) && html.contains("123 Main St"),
        "expected #project-civic-address to show the MENTION's address (mention wins \
         over the canonical projects row), got: {html}"
    );
    assert!(
        !html.contains("999 Canonical Fallback Ave"),
        "the canonical address must not appear when the mention has its own, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-type""#) && html.contains("residential"),
        "expected #project-type to show the mention's project_type, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-category""#) && html.contains("Residential"),
        "expected #project-category with the public taxonomy's EN label, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-reference-number""#) && html.contains("REF-2026-001"),
        "expected #project-reference-number, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-scale-units""#) && html.contains('42'),
        "expected #project-scale-units, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-scale-gfa""#) && html.contains("1234.5"),
        "expected #project-scale-gfa, got: {html}"
    );
    assert!(
        html.contains(r#"id="project-scale-storeys""#) && html.contains('12'),
        "expected #project-scale-storeys, got: {html}"
    );
}
```

- [ ] **Step 3: Add the "all fields null" test**

```rust
/// A project with no canonical address/type, no category, and no mention
/// at all must omit every one of the new rows entirely — no empty labels,
/// no literal "None", and the rest of the page (confidence notice,
/// timeline empty state) still renders normally. Seeded via a direct
/// INSERT (not `seed_project`, which requires non-null address/type) so
/// `civic_address_normalized`/`project_type` are genuinely NULL, not just
/// absent from a mention.
#[sqlx::test(migrations = "./migrations")]
async fn project_detail_new_fields_omitted_when_absent(pool: PgPool) {
    let project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) \
         VALUES (NULL, NULL) RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    for id in [
        "project-name",
        "project-civic-address",
        "project-type",
        "project-category",
        "project-reference-number",
        "project-scale-units",
        "project-scale-gfa",
        "project-scale-storeys",
    ] {
        assert!(
            !html.contains(&format!(r#"id="{id}""#)),
            "expected #{id} to be omitted entirely when its data is absent, got: {html}"
        );
    }
    assert!(
        !html.contains(">None<"),
        "missing fields must never render as a literal None, got: {html}"
    );
}
```

- [ ] **Step 4: Add the "non-public category is hidden" test**

```rust
/// A project whose `category_code` points at a `category_taxonomy` row
/// with `is_public = false` must not show a category row at all, even
/// though `category_code` itself is set on the project.
#[sqlx::test(migrations = "./migrations")]
async fn project_detail_non_public_category_is_hidden(pool: PgPool) {
    let project_id = seed_project(&pool, "77 internal category way", "commercial").await;
    sqlx::query!(
        "INSERT INTO category_taxonomy (code, label_en, label_fr, is_public) \
         VALUES ('internal-only', 'Internal Only', 'Interne Seulement', false)"
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE projects SET category_code = 'internal-only' WHERE id = $1",
        project_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        !html.contains(r#"id="project-category""#),
        "a non-public category must not render a #project-category row, got: {html}"
    );
}
```

- [ ] **Step 5: Run the new tests and confirm they fail for the expected reason**

Run: `cargo nextest run --workspace project_detail_all_new_fields_render_when_present project_detail_new_fields_omitted_when_absent project_detail_non_public_category_is_hidden`
Expected: all three FAIL — the first two on missing `#project-name`/etc. elements (template doesn't render them yet), the third trivially passes today (no `#project-category` exists at all yet) but re-run it after Task 3 to confirm it still passes for the *right* reason. If the crate doesn't compile because `insert_mention_with_details` is unused by only tests 1/3 (test 2 doesn't call it) — that's fine, `#[allow(dead_code)]` is not needed since all three tests are compiled into the same test binary and the helper is used.

- [ ] **Step 6: Commit**

```bash
git add apps/web/web/tests/timeline_resolver.rs
git commit -m "$(cat <<'EOF'
test(web): add failing coverage for project detail full-info fields

EOF
)"
```

---

### Task 2: `Project` struct and consolidated query, wired into the handler

**Files:**
- Modify: `apps/web/web/src/routes/projects.rs`

**Interfaces:**
- Consumes: `TimelineEvent`, `fetch_timeline_events`, `PrimaryCitationRow`, `fetch_primary_citation`, `core::synthesize_description`, `core::description_language_diverges`, `core::resolve_citation_view`, `core::canonical_url`, `core::build_signup_deep_link`, `core::format_detection_sentence`, `not_found_labels`, `cta_labels`, `timeline_labels` — all unchanged, already defined in this file.
- Produces: `struct Project { .. }`, `async fn fetch_project(db: &sqlx::PgPool, project_id: Uuid) -> Result<Option<Project>, sqlx::Error>`, `struct ProjectFieldLabels { .. }`, `fn project_field_labels(lang: &str) -> ProjectFieldLabels` — all used by Task 3 (template) via the `context!` macro's new keys (`project_name`, `civic_address`, `project_type`, `category_label`, `reference_number`, `scale_units`, `scale_gfa_sqm`, `scale_storeys`, and their matching `*_label` keys).

This task makes the page still compile and pass all *existing* tests, but Task 1's three new tests still fail until Task 3 adds the template markup — expected, matching this file's own established pattern.

- [ ] **Step 1: Replace `LatestMentionForDescription`/`fetch_latest_mention_for_description` with `Project`/`fetch_project`**

Find this block (currently lines 680–710):

```rust
struct LatestMentionForDescription {
    civic_address: Option<String>,
    project_type: Option<String>,
    scale_units: Option<i32>,
    scale_gfa_sqm: Option<f64>,
    scale_storeys: Option<i32>,
    approval_status_raw: Option<String>,
    source_url: Option<String>,
    language: Option<String>,
}

async fn fetch_latest_mention_for_description(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Option<LatestMentionForDescription>, sqlx::Error> {
    sqlx::query_as!(
        LatestMentionForDescription,
        "SELECT pm.civic_address, pm.project_type, pm.scale_units, pm.scale_gfa_sqm, \
                pm.scale_storeys, pm.approval_status_raw, sd.source_url, dc.language \
         FROM project_timeline_events pte \
         JOIN project_mentions pm ON pm.id = pte.project_mention_id \
         JOIN document_chunks dc ON dc.id = pm.document_chunk_id \
         JOIN source_documents sd ON sd.id = dc.source_document_id \
         WHERE pte.project_id = $1 \
         ORDER BY pte.created_at DESC \
         LIMIT 1",
        project_id
    )
    .fetch_optional(db)
    .await
}
```

Replace it with:

```rust
/// The project's full detail-page data in one query: the canonical
/// `projects` row's own columns, its public category label (if any,
/// joined with `category_taxonomy.is_public`), and its most recent
/// mention's descriptive/scale fields. "Most recent mention" is resolved
/// via `project_timeline_events` (`ORDER BY pte.created_at DESC LIMIT 1`),
/// the same table `fetch_timeline_events` uses — NOT a direct
/// `project_mentions.project_id` filter (see `fetch_timeline_events`'s doc
/// comment for why).
///
/// `mention_id` is `Some` only when the project has at least one timeline
/// event; it exists purely as a presence marker so callers can distinguish
/// "no mention at all" from "mention exists but every one of its fields
/// happens to be null" — the same distinction the former
/// `LatestMentionForDescription`-based code drew via `Option<LatestMentionForDescription>`
/// itself. `mention_civic_address`/`mention_project_type` are therefore
/// deliberately NOT coalesced with the canonical `projects` row here (the
/// description-synthesis call site needs the mention's own value only,
/// unmixed with canonical data, to preserve existing behavior); the
/// canonical-row fallback for the *displayed* address/type fields is
/// computed by the caller instead.
struct Project {
    mention_id: Option<Uuid>,
    mention_civic_address: Option<String>,
    mention_project_type: Option<String>,
    canonical_civic_address: Option<String>,
    canonical_project_type: Option<String>,
    project_name: Option<String>,
    reference_number: Option<String>,
    scale_units: Option<i32>,
    scale_gfa_sqm: Option<f64>,
    scale_storeys: Option<i32>,
    approval_status_raw: Option<String>,
    source_url: Option<String>,
    language: Option<String>,
    category_code: Option<String>,
    category_label_en: Option<String>,
    category_label_fr: Option<String>,
    confidence_level: Option<String>,
    merged_into_id: Option<Uuid>,
    first_detected_at: Option<DateTime<Utc>>,
    source_count: Option<i64>,
}

async fn fetch_project(db: &sqlx::PgPool, project_id: Uuid) -> Result<Option<Project>, sqlx::Error> {
    sqlx::query_as!(
        Project,
        r#"
        SELECT
            m.mention_id,
            m.civic_address AS mention_civic_address,
            m.project_type AS mention_project_type,
            p.civic_address_normalized AS canonical_civic_address,
            p.project_type AS canonical_project_type,
            m.project_name,
            m.reference_number,
            m.scale_units,
            m.scale_gfa_sqm,
            m.scale_storeys,
            m.approval_status_raw,
            m.source_url,
            m.language,
            p.category_code,
            ct.label_en AS category_label_en,
            ct.label_fr AS category_label_fr,
            p.confidence_level,
            p.merged_into_id,
            psd.first_detected_at,
            psd.source_count
        FROM projects p
        LEFT JOIN category_taxonomy ct
            ON ct.code = p.category_code AND ct.is_public
        LEFT JOIN LATERAL (
            SELECT pm.id AS mention_id, pm.project_name, pm.civic_address, pm.project_type,
                   pm.reference_number, pm.scale_units, pm.scale_gfa_sqm, pm.scale_storeys,
                   pm.approval_status_raw, sd.source_url, dc.language
            FROM project_timeline_events pte
            JOIN project_mentions pm ON pm.id = pte.project_mention_id
            JOIN document_chunks dc ON dc.id = pm.document_chunk_id
            JOIN source_documents sd ON sd.id = dc.source_document_id
            WHERE pte.project_id = p.id
            ORDER BY pte.created_at DESC
            LIMIT 1
        ) m ON true
        LEFT JOIN public_search_documents psd ON psd.project_id = p.id
        WHERE p.id = $1
        "#,
        project_id
    )
    .fetch_optional(db)
    .await
}
```

- [ ] **Step 2: Add `ProjectFieldLabels`/`project_field_labels`**

Add directly below `fn cta_labels(lang: &str) -> CtaLabels { ... }` (its closing brace):

```rust
/// EN/FR labels for the "Project details" section (project name, civic
/// address, project type, category, reference number, scale), added per
/// docs/superpowers/specs/2026-07-26-project-detail-completeness-design.md.
/// Mirrors `not_found_labels`/`cta_labels`'s plain lang-keyed
/// literal-struct pattern (no I/O).
struct ProjectFieldLabels {
    project_name_label: &'static str,
    civic_address_label: &'static str,
    project_type_label: &'static str,
    category_label: &'static str,
    reference_number_label: &'static str,
    scale_units_label: &'static str,
    scale_gfa_label: &'static str,
    scale_storeys_label: &'static str,
}

fn project_field_labels(lang: &str) -> ProjectFieldLabels {
    match lang {
        "fr" => ProjectFieldLabels {
            project_name_label: "Nom du projet : ",
            civic_address_label: "Adresse : ",
            project_type_label: "Type de projet : ",
            category_label: "Catégorie : ",
            reference_number_label: "Numéro de référence : ",
            scale_units_label: "Unités : ",
            scale_gfa_label: "Superficie de plancher (m²) : ",
            scale_storeys_label: "Étages : ",
        },
        _ => ProjectFieldLabels {
            project_name_label: "Project name: ",
            civic_address_label: "Address: ",
            project_type_label: "Project type: ",
            category_label: "Category: ",
            reference_number_label: "Reference number: ",
            scale_units_label: "Units: ",
            scale_gfa_label: "Floor area (m²): ",
            scale_storeys_label: "Storeys: ",
        },
    }
}
```

- [ ] **Step 3: Add the new fields to `ProjectDetailContext`**

Find (currently lines 825–858, the `struct ProjectDetailContext { ... }` block) and add these fields right after the opening brace (before `description: Option<String>,`):

```rust
    // Added per docs/superpowers/specs/2026-07-26-project-detail-completeness-design.md:
    // the "Project details" section. `civic_address`/`project_type` prefer
    // the most recent mention's value, falling back to the canonical
    // `projects` row when the mention doesn't have one. `category_label` is
    // `None` whenever the project has no category, or its category isn't
    // public.
    project_name: Option<String>,
    civic_address: Option<String>,
    project_type: Option<String>,
    category_label: Option<String>,
    reference_number: Option<String>,
    scale_units: Option<i32>,
    scale_gfa_sqm: Option<f64>,
    scale_storeys: Option<i32>,
```

- [ ] **Step 4: Replace the handler's `project_row` query and mention fetch**

Find (inside `get_project_detail_page`):

```rust
    let project_row = match sqlx::query!(
        r#"
        SELECT p.id, p.confidence_level, p.merged_into_id,
               psd.first_detected_at, psd.source_count
        FROM projects p
        LEFT JOIN public_search_documents psd ON psd.project_id = p.id
        WHERE p.id = $1
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(_) => return render_error_page(&labels, id),
    };
    let Some(project_row) = project_row else {
        return render_page_error(StatusCode::NOT_FOUND, not_found.title, not_found.message);
    };
```

Replace with:

```rust
    let project_row = match fetch_project(&state.db, id).await {
        Ok(row) => row,
        Err(_) => return render_error_page(&labels, id),
    };
    let Some(project_row) = project_row else {
        return render_page_error(StatusCode::NOT_FOUND, not_found.title, not_found.message);
    };
```

Find:

```rust
    let events = match fetch_timeline_events(&state.db, id).await {
        Ok(events) => events,
        Err(_) => return render_error_page(&labels, id),
    };

    let mention = match fetch_latest_mention_for_description(&state.db, id).await {
        Ok(mention) => mention,
        Err(_) => return render_error_page(&labels, id),
    };

    // IMP-REQ-005-03: description is synthesized (not stored) from the
    // project's most recent mention, so it's only present when a mention
    // exists at all (TC-005-1 vs. TC-005-2's "no mention seeded" case).
    // `source_document_url` is likewise derived at read time from that same
    // mention's document chunk's source document, rather than read from
    // `projects.source_document_url` (migration 020's column, which nothing
    // populates yet) — see `LatestMentionForDescription`.
    let description = mention.as_ref().map(|m| {
        core::synthesize_description(
            m.civic_address.as_deref(),
            m.project_type.as_deref(),
            m.scale_units,
            m.scale_gfa_sqm,
            m.scale_storeys,
            m.approval_status_raw.as_deref(),
        )
    });
    let description_lang = mention.as_ref().and_then(|m| m.language.clone());
    let description_language_diverges =
        core::description_language_diverges(lang, description_lang.as_deref());
```

Replace with:

```rust
    let events = match fetch_timeline_events(&state.db, id).await {
        Ok(events) => events,
        Err(_) => return render_error_page(&labels, id),
    };

    // IMP-REQ-005-03 (unchanged): description is synthesized (not stored)
    // from the project's most recent mention, so it's only present when a
    // mention exists at all (`mention_id` is the presence marker —
    // TC-005-1 vs. TC-005-2's "no mention seeded" case). Deliberately uses
    // the mention's OWN civic_address/project_type, not the canonical-row
    // fallback used for the "Project details" section below, so this
    // synthesis behaves exactly as it did before this pass.
    let description = project_row.mention_id.map(|_| {
        core::synthesize_description(
            project_row.mention_civic_address.as_deref(),
            project_row.mention_project_type.as_deref(),
            project_row.scale_units,
            project_row.scale_gfa_sqm,
            project_row.scale_storeys,
            project_row.approval_status_raw.as_deref(),
        )
    });
    let description_lang = project_row.language.clone();
    let description_language_diverges =
        core::description_language_diverges(lang, description_lang.as_deref());

    // docs/superpowers/specs/2026-07-26-project-detail-completeness-design.md:
    // "Project details" section fields. civic_address/project_type prefer
    // the mention's own value, falling back to the canonical projects row.
    let field_labels = project_field_labels(lang);
    let display_civic_address = project_row
        .mention_civic_address
        .clone()
        .or_else(|| project_row.canonical_civic_address.clone());
    let display_project_type = project_row
        .mention_project_type
        .clone()
        .or_else(|| project_row.canonical_project_type.clone());
    let category_label = if lang == "fr" {
        project_row.category_label_fr.clone()
    } else {
        project_row.category_label_en.clone()
    };
```

- [ ] **Step 5: Replace `source_document_url`/`confidence_level` reads and update `ProjectDetailContext` construction**

Find:

```rust
    let detail_context = ProjectDetailContext {
        description,
        confidence_level: project_row.confidence_level,
        source_document_url: mention.as_ref().and_then(|m| m.source_url.clone()),
        description_lang,
```

Replace with:

```rust
    let detail_context = ProjectDetailContext {
        description,
        confidence_level: project_row.confidence_level.clone(),
        source_document_url: project_row.source_url.clone(),
        description_lang,
        project_name: project_row.project_name.clone(),
        civic_address: display_civic_address,
        project_type: display_project_type,
        category_label,
        reference_number: project_row.reference_number.clone(),
        scale_units: project_row.scale_units,
        scale_gfa_sqm: project_row.scale_gfa_sqm,
        scale_storeys: project_row.scale_storeys,
```

(This inserts the eight new fields right after `description_lang,` and before the existing `citation_url: citation_view.citation_url,` line, which stays unchanged immediately after.)

- [ ] **Step 6: Add the new fields and labels to the `context!` render call**

Find:

```rust
            canonical_url => detail_context.canonical_url,
            detection_sentence => detail_context.detection_sentence,
            copy_link_label => labels.copy_link_label,
```

Replace with:

```rust
            canonical_url => detail_context.canonical_url,
            detection_sentence => detail_context.detection_sentence,
            project_name => detail_context.project_name,
            civic_address => detail_context.civic_address,
            civic_address_label => field_labels.civic_address_label,
            project_type => detail_context.project_type,
            project_type_label => field_labels.project_type_label,
            project_name_label => field_labels.project_name_label,
            category_label => detail_context.category_label,
            category_label_heading => field_labels.category_label,
            reference_number => detail_context.reference_number,
            reference_number_label => field_labels.reference_number_label,
            scale_units => detail_context.scale_units,
            scale_units_label => field_labels.scale_units_label,
            scale_gfa_sqm => detail_context.scale_gfa_sqm,
            scale_gfa_label => field_labels.scale_gfa_label,
            scale_storeys => detail_context.scale_storeys,
            scale_storeys_label => field_labels.scale_storeys_label,
            copy_link_label => labels.copy_link_label,
```

- [ ] **Step 7: Compile and run the existing test suite (expect Task 1's 2 new tests to still fail on missing template markup; everything else must still pass)**

Run: `cargo build --workspace`
Expected: builds cleanly (no unused-`LatestMentionForDescription`/`fetch_latest_mention_for_description` references remain anywhere in the crate — grep to confirm: `grep -rn "LatestMentionForDescription\|fetch_latest_mention_for_description" apps/web/web/src` should return nothing).

Run: `cargo nextest run --workspace`
Expected: every test in `timeline_resolver.rs`, `cta_upsell.rs`, `shareable_url.rs`, `search_integration.rs`, etc. that existed before this task still PASSES. `project_detail_all_new_fields_render_when_present` and `project_detail_new_fields_omitted_when_absent` (Task 1) still FAIL (no template markup yet). `project_detail_non_public_category_is_hidden` (Task 1) PASSES (trivially — no `#project-category` exists yet either way).

- [ ] **Step 8: Commit**

```bash
git add apps/web/web/src/routes/projects.rs
git commit -m "$(cat <<'EOF'
feat(web): consolidate project detail queries into one Project struct

EOF
)"
```

---

### Task 3: Render the new fields in `project_detail.html`

**Files:**
- Modify: `apps/web/templates/project_detail.html`

**Interfaces:**
- Consumes: the `context!` keys added in Task 2 Step 6 (`project_name`, `civic_address`, `civic_address_label`, `project_type`, `project_type_label`, `project_name_label`, `category_label`, `category_label_heading`, `reference_number`, `reference_number_label`, `scale_units`, `scale_units_label`, `scale_gfa_sqm`, `scale_gfa_label`, `scale_storeys`, `scale_storeys_label`).
- Produces: rendered `#project-name`, `#project-civic-address`, `#project-type`, `#project-category`, `#project-reference-number`, `#project-scale-units`, `#project-scale-gfa`, `#project-scale-storeys` elements, each independently omitted when its underlying value is absent — the ids Task 1's tests assert against.

- [ ] **Step 1: Insert the new rows into `.project-detail-fields`**

Find (lines 196–199 today):

```html
    <div class="project-detail-fields">
    {% if description is defined and description %}
    <p id="project-description">{{ description }}</p>
    {% endif %}
```

Replace with:

```html
    <div class="project-detail-fields">
    {% if project_name is defined and project_name %}
    <p id="project-name">{{ project_name_label | default("Project name: ", true) }}{{ project_name }}</p>
    {% endif %}

    {% if civic_address is defined and civic_address %}
    <p id="project-civic-address">{{ civic_address_label | default("Address: ", true) }}{{ civic_address }}</p>
    {% endif %}

    {% if project_type is defined and project_type %}
    <p id="project-type">{{ project_type_label | default("Project type: ", true) }}{{ project_type }}</p>
    {% endif %}

    {% if category_label is defined and category_label %}
    <p id="project-category">{{ category_label_heading | default("Category: ", true) }}{{ category_label }}</p>
    {% endif %}

    {% if reference_number is defined and reference_number %}
    <p id="project-reference-number">{{ reference_number_label | default("Reference number: ", true) }}{{ reference_number }}</p>
    {% endif %}

    {% if scale_units is defined and scale_units %}
    <p id="project-scale-units">{{ scale_units_label | default("Units: ", true) }}{{ scale_units }}</p>
    {% endif %}

    {% if scale_gfa_sqm is defined and scale_gfa_sqm %}
    <p id="project-scale-gfa">{{ scale_gfa_label | default("Floor area (m²): ", true) }}{{ scale_gfa_sqm }}</p>
    {% endif %}

    {% if scale_storeys is defined and scale_storeys %}
    <p id="project-scale-storeys">{{ scale_storeys_label | default("Storeys: ", true) }}{{ scale_storeys }}</p>
    {% endif %}

    {% if description is defined and description %}
    <p id="project-description">{{ description }}</p>
    {% endif %}
```

- [ ] **Step 2: Run Task 1's tests and confirm they now pass**

Run: `cargo nextest run --workspace project_detail_all_new_fields_render_when_present project_detail_new_fields_omitted_when_absent project_detail_non_public_category_is_hidden`
Expected: all three PASS.

- [ ] **Step 3: Run the full test suite**

Run: `cargo nextest run --workspace`
Expected: every test passes, including all pre-existing `timeline_resolver.rs`/`search_integration.rs`/`cta_upsell.rs`/`shareable_url.rs`/`admin_routes.rs`/`no_account_required.rs` tests — this change is additive to the template and must not alter any existing element's markup or the `.project-detail-fields` wrapper's existing children order relative to each other (description/confidence-notice/description-language-notice/source-document-link keep their exact relative order, per `imp_req_005_09_detail_fields_have_responsive_wrapper_class`'s wrapper-content assertions).

- [ ] **Step 4: Commit**

```bash
git add apps/web/templates/project_detail.html
git commit -m "$(cat <<'EOF'
feat(web): render project name, address, type, category, reference number, and scale on the project detail page

EOF
)"
```

---

## Self-Review Notes

- **Spec coverage:** every field the spec lists (project_name, civic_address, project_type, category_label, reference_number, scale_units, scale_gfa_sqm, scale_storeys) has a query column (Task 2), a context key (Task 2), and a template row (Task 3). Citation isolation is untouched (Task 2 never modifies `fetch_primary_citation`). Null-omission and category `is_public` gating are both covered by Task 1's tests.
- **Placeholder scan:** no TBDs; every step has literal code, not descriptions of code.
- **Type consistency:** `Project`'s field names match exactly what Task 2 Steps 4–6 reference (`mention_id`, `mention_civic_address`, `mention_project_type`, `canonical_civic_address`, `canonical_project_type`, `project_name`, `reference_number`, `scale_units`, `scale_gfa_sqm`, `scale_storeys`, `approval_status_raw`, `source_url`, `language`, `category_code`, `category_label_en`, `category_label_fr`, `confidence_level`, `merged_into_id`, `first_detected_at`, `source_count`); `ProjectDetailContext`'s new fields match the `context!` keys in Step 6 and the template variables in Task 3 Step 1.
