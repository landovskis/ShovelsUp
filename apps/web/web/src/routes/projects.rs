use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use chrono::{DateTime, Utc};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    routes::locale::{extract_cookie_value, resolve_ui_locale},
    AppState,
};

/// IMP-REQ-005-03: pure, I/O-free synthesis of the project-detail page's
/// description and language-divergence decision. Mirrors
/// `search::core::synthesize_display_name`'s pattern (a `core` submodule of
/// data-in/data-out functions, no DB/HTTP/clock access) but produces a
/// fuller sentence since this is the detail page's actual description, not
/// a short search-result label.
mod core {
    /// Synthesizes a one-sentence project description from a project's most
    /// recent mention's extracted fields. There is no free-text
    /// `description` column anywhere in `projects`/`project_mentions`/the
    /// pipeline (confirmed in migration 020's doc comment) — this builds a
    /// reasonable sentence out of whatever structured fields the mention
    /// actually carries. Callers only invoke this when a mention exists at
    /// all (see `ProjectDetailContext`'s caller); a mention with all-`None`
    /// sub-fields still yields a minimal but real, non-empty sentence.
    pub fn synthesize_description(
        civic_address: Option<&str>,
        project_type: Option<&str>,
        scale_units: Option<i32>,
        scale_gfa_sqm: Option<f64>,
        scale_storeys: Option<i32>,
        approval_status_raw: Option<&str>,
    ) -> String {
        let mut sentence = String::from("A");
        if let Some(project_type) = non_blank(project_type) {
            sentence.push(' ');
            sentence.push_str(project_type);
        }
        sentence.push_str(" project");

        if let Some(civic_address) = non_blank(civic_address) {
            sentence.push_str(" at ");
            sentence.push_str(civic_address);
        }

        let mut scale_parts = Vec::new();
        if let Some(units) = scale_units {
            scale_parts.push(format!("{units} unit{}", if units == 1 { "" } else { "s" }));
        }
        if let Some(storeys) = scale_storeys {
            scale_parts.push(format!(
                "{storeys} storey{}",
                if storeys == 1 { "" } else { "s" }
            ));
        }
        if let Some(gfa) = scale_gfa_sqm {
            scale_parts.push(format!("{gfa:.0} m² gross floor area"));
        }
        if !scale_parts.is_empty() {
            sentence.push_str(" (");
            sentence.push_str(&scale_parts.join(", "));
            sentence.push(')');
        }

        if let Some(status) = non_blank(approval_status_raw) {
            sentence.push_str(". Status: ");
            sentence.push_str(status);
        }

        sentence.push('.');
        sentence
    }

    /// Trims a field and returns `None` for empty/whitespace-only values,
    /// same convention as `search::core`'s blank-field handling.
    fn non_blank(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|s| !s.is_empty())
    }

    /// Decides whether the description-language divergence notice should
    /// render: only when both the resolved UI locale and the source
    /// mention's document-chunk language are known, and they differ. A
    /// `None` chunk language (no language signal recorded, or no mention at
    /// all) means "no divergence to report", not "assume divergence".
    pub fn description_language_diverges(ui_lang: &str, mention_language: Option<&str>) -> bool {
        matches!(mention_language, Some(lang) if lang != ui_lang)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn synthesize_description_full_data_reads_naturally() {
            assert_eq!(
                synthesize_description(
                    Some("100 fulldata ave"),
                    Some("residential"),
                    None,
                    None,
                    None,
                    None,
                ),
                "A residential project at 100 fulldata ave."
            );
        }

        #[test]
        fn synthesize_description_includes_scale_and_status_when_present() {
            assert_eq!(
                synthesize_description(
                    Some("1 tower rd"),
                    Some("commercial"),
                    Some(1),
                    Some(2500.0),
                    Some(12),
                    Some("approved"),
                ),
                "A commercial project at 1 tower rd (1 unit, 12 storeys, 2500 m² gross floor area). Status: approved."
            );
        }

        #[test]
        fn synthesize_description_pluralizes_units_and_storeys() {
            assert_eq!(
                synthesize_description(None, None, Some(3), None, Some(2), None),
                "A project (3 units, 2 storeys)."
            );
        }

        #[test]
        fn synthesize_description_blank_project_type_and_address_omitted() {
            assert_eq!(
                synthesize_description(Some("   "), Some(""), None, None, None, None),
                "A project."
            );
        }

        #[test]
        fn synthesize_description_minimal_data_still_non_empty() {
            assert_eq!(
                synthesize_description(None, None, None, None, None, None),
                "A project."
            );
        }

        #[test]
        fn description_language_diverges_true_when_languages_differ() {
            assert!(description_language_diverges("en", Some("fr")));
        }

        #[test]
        fn description_language_diverges_false_when_languages_match() {
            assert!(!description_language_diverges("en", Some("en")));
        }

        #[test]
        fn description_language_diverges_false_when_no_signal() {
            assert!(!description_language_diverges("en", None));
        }
    }
}

#[derive(Serialize)]
pub struct TimelineEvent {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_mention_id: Uuid,
    pub event_date: DateTime<Utc>,
    pub normalized_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// GET /api/v1/projects/{id}/timeline.
///
/// Events are ordered chronologically; `created_at` provides stable
/// ingestion-order sequencing for equal event timestamps.
pub async fn get_project_timeline(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TimelineEvent>>, StatusCode> {
    let project_exists = sqlx::query_scalar!("SELECT id FROM projects WHERE id = $1", id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    project_exists.ok_or(StatusCode::NOT_FOUND)?;

    let events = fetch_timeline_events(&state.db, id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Json(events))
}

async fn fetch_timeline_events(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Vec<TimelineEvent>, sqlx::Error> {
    let events = sqlx::query!(
        "SELECT id, project_id, project_mention_id, event_date, normalized_status, created_at \
         FROM project_timeline_events WHERE project_id = $1 \
         ORDER BY event_date ASC, created_at ASC",
        project_id
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|event| TimelineEvent {
        id: event.id,
        project_id: event.project_id,
        project_mention_id: event.project_mention_id,
        event_date: event.event_date,
        normalized_status: event.normalized_status,
        created_at: event.created_at,
    })
    .collect();

    Ok(events)
}

/// The project-detail page's most recent mention, joined through to its
/// source document chunk. "Latest" is resolved via `project_timeline_events`
/// (`ORDER BY pte.created_at DESC LIMIT 1`), the same table
/// `fetch_timeline_events` above already uses to associate a project with
/// its mentions — NOT a direct `project_mentions.project_id` filter: that
/// column is only populated by the pipeline's `resolver::link_mention` in
/// production (see `pipeline/src/resolver/mod.rs`), and this repo's test
/// fixtures (`insert_mention` in `tests/timeline_resolver.rs`) deliberately
/// exercise the resolver-free path, linking project<->mention only through
/// `project_timeline_events`, mirroring `fetch_timeline_events`'s own join.
/// `None` means the project has no timeline event (and therefore no
/// resolvable mention) at all yet (TC-005-2's "graceful degradation" case).
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

/// REQ-005 scaffolding stub: shape of the additional descriptive fields
/// TC-005-1/-2/-5 assert against in rendered `project_detail.html` output.
/// `projects` has no `description_lang`/`confidence_level`/
/// `source_document_url` columns yet (that migration is out of scope for
/// this pass) — IMP-REQ-005-05 must add the migration, populate this
/// struct from a real query, and thread it into `get_project_detail_page`'s
/// template context (rendering `#project-description`,
/// `#confidence-notice`, `#source-document-link`, and, when
/// `description_lang` diverges from the resolved UI `lang`, a
/// `#description-language-notice` indicator) so TC-005-1/-2/-5 pass.
///
/// REQ-006 scaffolding stub: `citation_url`, `citation_url_reliable`, and
/// `citation_meeting_date` are the additional fields TC-006-1..5 assert
/// against. `source_documents` has no `meeting_date` or
/// `citation_url_reliable` columns yet — IMP-REQ-006-05's migration must
/// add both, IMP-REQ-006-05's query must join through
/// `project_mentions.document_chunk_id -> document_chunks.source_document_id
/// -> source_documents` to populate this struct (a project may have zero or
/// many associated source documents; this stub assumes "most recent" or
/// similar selection is IMP-REQ-006-05's call), and IMP-REQ-006-06 must
/// thread it into `get_project_detail_page`'s template context so
/// IMP-REQ-006-08's template can render an `#project-source` section:
/// a hyperlink to `citation_url` (with `citation_meeting_date`) when
/// `citation_url_reliable` is `Some(true)`, citation-only text (no link)
/// when `Some(false)`, a "Document retrieved" fallback when
/// `citation_meeting_date` is `None`, the section omitted entirely when no
/// source document exists, and — per IMP-REQ-006-05/-06 — the citation
/// lookup query's failure must be isolated (caught and degraded
/// gracefully) rather than propagated to a page-wide 503, unlike the
/// project-existence and timeline queries above.
///
/// REQ-013 scaffolding stub: `merged_into_id` and `canonical_url` are the
/// additional fields TC-013-1/-2 assert against. `projects` has no
/// `merged_into_id` column yet — IMP-REQ-013-01's migration must add it
/// (nullable, self-referencing FK to `projects.id`), and
/// `get_project_detail_page` must query it before any other lookup: when
/// `Some`, IMP-REQ-013-04 requires a 301 redirect to
/// `/projects/{merged_into_id}` rather than rendering this project's own
/// page at all (see TC-013-1). `canonical_url` must be built by
/// IMP-REQ-013-06 from a configured base URL (not yet present anywhere in
/// `AppState`/`main.rs`/`.env.example` — that configuration is also
/// IMP-REQ-013-01's job), never from the request's `Host` or
/// `X-Forwarded-Host` header, and threaded into `project_detail.html` as a
/// `<link rel="canonical" href="...">` tag (see TC-013-2, which
/// deliberately does not hardcode the exact base-URL literal since it
/// doesn't exist yet).
/// REQ-014 scaffolding note (no new struct fields needed): TC-014-1..6 in
/// `tests/cta_upsell.rs` assert against a CTA card that is purely static
/// markup/copy/client-JS, not per-project data, so nothing here needs a
/// new column/query — this requirement's Loop B work is template +
/// static-asset + a brand-new `POST /api/v1/cta-events` route (backed by
/// a `cta_events` table migration that doesn't exist yet; both out of
/// scope for this pass). `get_project_detail_page`/`project_detail.html`
/// must add, unconditionally (this app has no auth, so every view is
/// anonymous per TC-014-1): a card container `id="cta-upsell"` (never a
/// `<dialog>`/`role="dialog"`/scroll-lock class/backdrop — TC-014-2), a
/// same-tab `id="cta-signup-link"` anchor with `href="/signup"` and no
/// `target="_blank"`, reflecting no raw request input (TC-014-3), a
/// `id="cta-collapse-toggle"` button with `aria-expanded` (TC-014-4), and
/// an inline `<script>` that persists collapsed/expanded state in
/// `localStorage` under the key `cta-upsell-collapsed` for 30 days
/// (TC-014-6). The telemetry POST (TC-014-5) must remain a fully separate
/// route/handler so its failures never affect this page's own rendering.
///
/// REQ-015 scaffolding stub: `first_detected_at`/`source_count` are the
/// additional fields TC-015-2 asserts against (the detail-page half of the
/// "Detected N days ago from M council source(s)" indicator; see
/// `SearchResult::first_detected_at`/`source_count` in `routes/search.rs`
/// for the search-card half). Same backing gap: `public_search_documents`
/// has neither column yet (IMP-REQ-015-02/03/04's migration+materializer),
/// so `get_project_detail_page` has no query populating these, and
/// `project_detail.html` has no rendering for them at all — IMP-REQ-015-07
/// must query them and IMP-REQ-015-12 must thread them into the template,
/// rendering the indicator only when both are `Some` (TC-015-5).
struct ProjectDetailContext {
    description: Option<String>,
    confidence_level: Option<String>,
    source_document_url: Option<String>,
    // Populated for real (from the latest mention's document-chunk
    // language), but only its derived `description_language_diverges`
    // boolean (computed before this struct is built) is threaded into the
    // template today — the raw language tag itself isn't rendered anywhere
    // yet.
    #[allow(dead_code)]
    description_lang: Option<String>,
    #[allow(dead_code)]
    citation_url: Option<String>,
    #[allow(dead_code)]
    citation_url_reliable: Option<bool>,
    #[allow(dead_code)]
    citation_meeting_date: Option<chrono::DateTime<chrono::Utc>>,
    #[allow(dead_code)]
    merged_into_id: Option<Uuid>,
    #[allow(dead_code)]
    canonical_url: Option<String>,
    #[allow(dead_code)]
    first_detected_at: Option<chrono::DateTime<chrono::Utc>>,
    #[allow(dead_code)]
    source_count: Option<i64>,
}

struct TimelineLabels {
    page_title: &'static str,
    timeline_title: &'static str,
    timeline_error_message: &'static str,
    retry_label: &'static str,
    timeline_empty_message: &'static str,
    status_update_fallback: &'static str,
    nav_permits: &'static str,
    nav_council: &'static str,
    confidence_notice_label: &'static str,
    confidence_unassessed_fallback: &'static str,
    source_document_link_label: &'static str,
    description_language_notice: &'static str,
}

fn timeline_labels(lang: &str) -> TimelineLabels {
    match lang {
        "fr" => TimelineLabels {
            page_title: "Détails du projet",
            timeline_title: "Historique du projet",
            timeline_error_message: "Nous n’avons pas pu charger l’historique de ce projet.",
            retry_label: "Réessayer",
            timeline_empty_message: "Aucun événement n’a encore été enregistré pour ce projet.",
            status_update_fallback: "Mise à jour enregistrée",
            nav_permits: "Permis",
            nav_council: "Conseil",
            confidence_notice_label: "Niveau de confiance : ",
            confidence_unassessed_fallback: "pas encore évalué",
            source_document_link_label: "Voir le document source",
            description_language_notice: "Cette description provient d’un document dans une autre langue que l’interface.",
        },
        _ => TimelineLabels {
            page_title: "Project details",
            timeline_title: "Project timeline",
            timeline_error_message: "We couldn’t load this project’s timeline.",
            retry_label: "Retry timeline",
            timeline_empty_message: "No timeline events have been recorded for this project yet.",
            status_update_fallback: "Update recorded",
            nav_permits: "Permits",
            nav_council: "Council",
            confidence_notice_label: "Confidence level: ",
            confidence_unassessed_fallback: "not yet assessed",
            source_document_link_label: "View source document",
            description_language_notice: "This description is sourced from a document in a different language than this page.",
        },
    }
}

/// Query params accepted by `GET /projects/{id}` (IMP-REQ-005-04):
/// `lang` is the explicit UI-locale override, highest-precedence input to
/// `locale::resolve_ui_locale` — the same shared utility `get_search_page`
/// uses, so this page now honors `?lang=`/`lang` cookie/`Accept-Language`
/// precedence rather than only `Accept-Language` (the plain `detect_lang`
/// call this superseded).
#[derive(Debug, Deserialize)]
pub struct ProjectDetailParams {
    pub lang: Option<String>,
}

/// GET /projects/{id}.
///
/// Server-rendered project-detail page: renders the timeline inline (no
/// separate loading state, since the initial page load already has the
/// data) or the error state if the DB is unavailable, per TC-REQ-006-6.
pub async fn get_project_detail_page(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ProjectDetailParams>,
    headers: HeaderMap,
) -> Result<(StatusCode, Html<String>), StatusCode> {
    // IMP-REQ-005-04: explicit `?lang=` param > `lang` cookie >
    // `Accept-Language` header > `"en"` default, via the SAME shared
    // `locale::resolve_ui_locale` utility `get_search_page` uses (extracted
    // from `search.rs`'s `core` module), superseding the plain
    // `detect_lang(&headers)` call this page previously used, which only
    // ever consulted `Accept-Language`.
    let accept_language = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let cookie_lang = extract_cookie_value(&headers, "lang");
    let lang = resolve_ui_locale(params.lang.as_deref(), cookie_lang, accept_language);
    let labels = timeline_labels(lang);

    let tmpl = state
        .env
        .get_template("project_detail.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let render_error_page =
        |labels: &TimelineLabels| -> Result<(StatusCode, Html<String>), StatusCode> {
            let html = tmpl
                .render(context! {
                    lang => lang,
                    nav_permits => labels.nav_permits,
                    nav_council => labels.nav_council,
                    page_title => labels.page_title,
                    timeline_title => labels.timeline_title,
                    timeline_error => true,
                    timeline_error_message => labels.timeline_error_message,
                    retry_label => labels.retry_label,
                    timeline_retry_url => format!("/projects/{id}"),
                })
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok((StatusCode::SERVICE_UNAVAILABLE, Html(html)))
        };

    let project_row = match sqlx::query!(
        "SELECT id, confidence_level FROM projects WHERE id = $1",
        id
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(_) => return render_error_page(&labels),
    };
    let Some(project_row) = project_row else {
        return Err(StatusCode::NOT_FOUND);
    };

    let events = match fetch_timeline_events(&state.db, id).await {
        Ok(events) => events,
        Err(_) => return render_error_page(&labels),
    };

    let mention = match fetch_latest_mention_for_description(&state.db, id).await {
        Ok(mention) => mention,
        Err(_) => return render_error_page(&labels),
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

    let detail_context = ProjectDetailContext {
        description,
        confidence_level: project_row.confidence_level,
        source_document_url: mention.as_ref().and_then(|m| m.source_url.clone()),
        description_lang,
        citation_url: None,
        citation_url_reliable: None,
        citation_meeting_date: None,
        merged_into_id: None,
        canonical_url: None,
        first_detected_at: None,
        source_count: None,
    };

    let html = tmpl
        .render(context! {
            lang => lang,
            nav_permits => labels.nav_permits,
            nav_council => labels.nav_council,
            page_title => labels.page_title,
            timeline_title => labels.timeline_title,
            timeline_empty_message => labels.timeline_empty_message,
            status_update_fallback => labels.status_update_fallback,
            timeline_events => events,
            description => detail_context.description,
            confidence_level => detail_context.confidence_level,
            confidence_notice_label => labels.confidence_notice_label,
            confidence_unassessed_fallback => labels.confidence_unassessed_fallback,
            source_document_url => detail_context.source_document_url,
            source_document_link_label => labels.source_document_link_label,
            description_language_diverges => description_language_diverges,
            description_language_notice => labels.description_language_notice,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::OK, Html(html)))
}
