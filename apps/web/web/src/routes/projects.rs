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
    use chrono::{DateTime, Utc};

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

    /// IMP-REQ-006-03: the citation section's rendering decision, for a
    /// single project's most-recent source document. `municipality_name` is
    /// carried through unchanged (it's not part of the decision) purely so
    /// callers have one struct to thread into the template.
    #[derive(Debug, PartialEq)]
    pub struct CitationView {
        /// `None` means "no source document at all" (TC-006-4): the
        /// citation section must be omitted entirely. `Some` means a
        /// section must render, with the link/text-only choice driven by
        /// `is_reliable`.
        pub citation_url: Option<String>,
        pub municipality_name: Option<String>,
        /// URL-shape heuristic (IMP-REQ-006-04): `true` only when
        /// `citation_url` has neither a query string nor a path segment
        /// that looks session-scoped. Meaningless when `citation_url` is
        /// `None`.
        pub is_reliable: bool,
        pub meeting_date: Option<DateTime<Utc>>,
        /// `true` when a source document exists but has no recorded
        /// `meeting_date` (TC-006-3): the template must render a "Document
        /// retrieved" fallback instead of a date.
        pub document_retrieved_fallback: bool,
    }

    /// Pure decision function: given the primary source document's URL,
    /// municipality name, and meeting date (all `None` when no source
    /// document exists at all), decides how — or whether — the project
    /// detail page's citation section should render. No DB/HTTP access;
    /// callers (`fetch_primary_citation` + `get_project_detail_page`) do
    /// all I/O before calling this.
    pub fn resolve_citation_view(
        source_url: Option<&str>,
        municipality_name: Option<&str>,
        meeting_date: Option<DateTime<Utc>>,
    ) -> CitationView {
        match source_url {
            None => CitationView {
                citation_url: None,
                municipality_name: None,
                is_reliable: false,
                meeting_date: None,
                document_retrieved_fallback: false,
            },
            Some(url) => CitationView {
                citation_url: Some(url.to_string()),
                municipality_name: municipality_name.map(str::to_string),
                is_reliable: is_reliable_citation_url(url),
                meeting_date,
                document_retrieved_fallback: meeting_date.is_none(),
            },
        }
    }

    /// URL-shape reliability heuristic (IMP-REQ-006-04), directly justified
    /// by TC-006-1/-2's two examples: a plain URL like
    /// `https://test-city.example/reliable-doc` (no query string, no
    /// session-like path segment) is reliable; a session-scoped portal URL
    /// like `https://montreal.ca/portal/session/8f3c1?token=ephemeral` (has
    /// both a query string AND a literal `session` path segment) is not.
    /// Checking both signals — not just the query string — means a future
    /// session-scoped URL that omits the query string (a path-based session
    /// token, the risk the plan's own notes call out) still gets classified
    /// unreliable, without over-fitting to today's two examples. A
    /// malformed URL is treated as unreliable: we can't verify its shape,
    /// so the conservative choice is to not hyperlink it. Uses the `url`
    /// crate (IMP-REQ-006-04) for robust query-string/path-segment parsing
    /// rather than naive substring matching, which would be fooled by e.g.
    /// a literal `?` or `session` appearing inside a path segment's text.
    fn is_reliable_citation_url(url: &str) -> bool {
        match url::Url::parse(url) {
            Ok(parsed) => {
                let has_query_string = parsed.query().is_some();
                let has_session_segment = parsed
                    .path_segments()
                    .map(|mut segments| segments.any(|segment| segment.eq_ignore_ascii_case("session")))
                    .unwrap_or(false);
                !has_query_string && !has_session_segment
            }
            Err(_) => false,
        }
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

        // -- resolve_citation_view / is_reliable_citation_url (IMP-REQ-006-03) --

        #[test]
        fn resolve_citation_view_no_source_document_omits_citation() {
            let view = resolve_citation_view(None, None, None);
            assert_eq!(
                view,
                CitationView {
                    citation_url: None,
                    municipality_name: None,
                    is_reliable: false,
                    meeting_date: None,
                    document_retrieved_fallback: false,
                }
            );
        }

        #[test]
        fn resolve_citation_view_plain_url_is_reliable() {
            // TC-006-1's exact URL: no query string, no session-like path segment.
            let view = resolve_citation_view(
                Some("https://test-city.example/reliable-doc"),
                Some("Test City"),
                None,
            );
            assert!(view.is_reliable);
            assert_eq!(
                view.citation_url.as_deref(),
                Some("https://test-city.example/reliable-doc")
            );
        }

        #[test]
        fn resolve_citation_view_session_scoped_url_is_unreliable() {
            // TC-006-2's exact URL: query string AND a "session" path segment.
            let view = resolve_citation_view(
                Some("https://montreal.ca/portal/session/8f3c1?token=ephemeral"),
                Some("Test City"),
                None,
            );
            assert!(!view.is_reliable);
        }

        #[test]
        fn resolve_citation_view_session_path_without_query_string_is_unreliable() {
            // The plan's own risk note: a session-scoped URL that omits the
            // query string entirely must still be classified unreliable via
            // the path-segment signal alone.
            let view = resolve_citation_view(
                Some("https://montreal.ca/portal/session/8f3c1"),
                Some("Test City"),
                None,
            );
            assert!(!view.is_reliable);
        }

        #[test]
        fn resolve_citation_view_query_string_without_session_segment_is_unreliable() {
            let view = resolve_citation_view(
                Some("https://test-city.example/doc?v=2"),
                Some("Test City"),
                None,
            );
            assert!(!view.is_reliable);
        }

        #[test]
        fn resolve_citation_view_malformed_url_is_treated_as_unreliable() {
            let view = resolve_citation_view(Some("not a url"), Some("Test City"), None);
            assert!(!view.is_reliable);
        }

        #[test]
        fn resolve_citation_view_missing_meeting_date_sets_fallback_flag() {
            // TC-006-3: meeting_date is None.
            let view = resolve_citation_view(
                Some("https://test-city.example/undated-doc"),
                Some("Test City"),
                None,
            );
            assert!(view.document_retrieved_fallback);
        }

        #[test]
        fn resolve_citation_view_present_meeting_date_clears_fallback_flag() {
            let meeting_date = chrono::Utc::now();
            let view = resolve_citation_view(
                Some("https://test-city.example/dated-doc"),
                Some("Test City"),
                Some(meeting_date),
            );
            assert!(!view.document_retrieved_fallback);
            assert_eq!(view.meeting_date, Some(meeting_date));
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

/// IMP-REQ-006-05: the project's primary (most recent) source document's
/// citation-relevant data. Mirrors `fetch_latest_mention_for_description`'s
/// join pattern exactly (`project_timeline_events` -> `project_mentions` ->
/// `document_chunks` -> `source_documents`, "latest" = `ORDER BY
/// pte.created_at DESC LIMIT 1`) plus one further join to `municipalities`
/// for the citation text's municipality name. `None` means the project has
/// no associated source document at all (TC-006-4).
struct PrimaryCitationRow {
    source_url: String,
    meeting_date: Option<DateTime<Utc>>,
    municipality_name: String,
}

async fn fetch_primary_citation(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Option<PrimaryCitationRow>, sqlx::Error> {
    sqlx::query_as!(
        PrimaryCitationRow,
        "SELECT sd.source_url, sd.meeting_date, m.name AS municipality_name \
         FROM project_timeline_events pte \
         JOIN project_mentions pm ON pm.id = pte.project_mention_id \
         JOIN document_chunks dc ON dc.id = pm.document_chunk_id \
         JOIN source_documents sd ON sd.id = dc.source_document_id \
         JOIN municipalities m ON m.id = sd.municipality_id \
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
/// REQ-006 (IMP-REQ-006-03/-04/-05/-06, done): `citation_url`,
/// `citation_url_reliable`, `citation_meeting_date`,
/// `citation_municipality_name`, and `citation_document_retrieved_fallback`
/// are populated for real by `get_project_detail_page` from
/// `fetch_primary_citation` (joining `project_mentions.document_chunk_id ->
/// document_chunks.source_document_id -> source_documents ->
/// municipalities`, "most recent" resolved the same way
/// `fetch_latest_mention_for_description` does) piped through
/// `core::resolve_citation_view`'s pure decision. `citation_url` is `None`
/// when the project has no source document at all (TC-006-4), in which
/// case the template omits the `#project-source` section entirely.
/// Otherwise the template renders a hyperlink when `citation_url_reliable`
/// is `true` (TC-006-1), citation-only text (no link) when `false`
/// (TC-006-2), and a "Document retrieved" fallback when
/// `citation_document_retrieved_fallback` is `true` (TC-006-3). Per
/// IMP-REQ-006-06, the citation lookup's failure is isolated: it uses
/// `.ok()` rather than `?`/`map_err(_, SERVICE_UNAVAILABLE)`, so a DB error
/// on just this query degrades to "no citation section" instead of
/// propagating to a page-wide 503 — unlike the project-existence and
/// timeline queries above, which correctly still 503 on failure (TC-REQ-006-6).
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
    // IMP-REQ-006-06: populated for real from `fetch_primary_citation` +
    // `core::resolve_citation_view`. `citation_url` is `None` when the
    // project has no source document at all (TC-006-4), in which case the
    // other citation_* fields below are meaningless placeholders.
    citation_url: Option<String>,
    citation_url_reliable: bool,
    citation_meeting_date: Option<chrono::DateTime<chrono::Utc>>,
    citation_municipality_name: Option<String>,
    citation_document_retrieved_fallback: bool,
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
    document_retrieved_fallback_label: &'static str,
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
            document_retrieved_fallback_label: "Document récupéré",
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
            document_retrieved_fallback_label: "Document retrieved",
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

    // IMP-REQ-006-06: the citation lookup's failure is deliberately isolated
    // from the rest of the page via `.ok()` (never `?`/`SERVICE_UNAVAILABLE`)
    // — a DB error hitting just this query degrades to "no citation
    // section" rather than 503ing the whole page (TC-006-5), unlike the
    // project-existence/timeline queries above which correctly still 503 on
    // a full outage (TC-REQ-006-6). `citation_db_override` lets tests target
    // this query's pool independently of `state.db` (see AppState's doc
    // comment); production always uses `state.db` here since the override
    // is always `None`.
    let citation_pool = state.citation_db_override.as_ref().unwrap_or(&state.db);
    let citation_row = fetch_primary_citation(citation_pool, id).await.ok().flatten();
    let citation_view = core::resolve_citation_view(
        citation_row.as_ref().map(|row| row.source_url.as_str()),
        citation_row.as_ref().map(|row| row.municipality_name.as_str()),
        citation_row.as_ref().and_then(|row| row.meeting_date),
    );

    let detail_context = ProjectDetailContext {
        description,
        confidence_level: project_row.confidence_level,
        source_document_url: mention.as_ref().and_then(|m| m.source_url.clone()),
        description_lang,
        citation_url: citation_view.citation_url,
        citation_url_reliable: citation_view.is_reliable,
        citation_meeting_date: citation_view.meeting_date,
        citation_municipality_name: citation_view.municipality_name,
        citation_document_retrieved_fallback: citation_view.document_retrieved_fallback,
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
            citation_url => detail_context.citation_url,
            citation_url_reliable => detail_context.citation_url_reliable,
            citation_meeting_date => detail_context.citation_meeting_date,
            citation_municipality_name => detail_context.citation_municipality_name,
            citation_document_retrieved_fallback => detail_context.citation_document_retrieved_fallback,
            document_retrieved_fallback_label => labels.document_retrieved_fallback_label,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::OK, Html(html)))
}
