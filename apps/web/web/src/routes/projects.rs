use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
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
    use uuid::Uuid;

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
    /// IMP-REQ-013-06: builds the project detail page's canonical, shareable
    /// URL from a configured base URL (never from the request's own
    /// `Host`/`X-Forwarded-Host` header, which a client fully controls —
    /// TC-013-2's exact concern). Pure string composition: the shell
    /// (`get_project_detail_page`) is responsible for sourcing `base_url`
    /// from configuration (env var, with a production-safe default) and
    /// passing it in here; this function never reads the environment or
    /// request headers itself. `base_url` is expected without a trailing
    /// slash; a trailing slash is stripped defensively so a misconfigured
    /// value doesn't produce a doubled `//projects/...` path.
    pub fn canonical_url(base_url: &str, project_id: uuid::Uuid) -> String {
        format!("{}/projects/{project_id}", base_url.trim_end_matches('/'))
    }

    /// IMP-REQ-014-04: builds the CTA card's `/signup` deep link from the
    /// already-parsed, server-validated project id (a `Uuid`, never the raw
    /// `:id` path segment or any query string) — TC-014-3's exact concern
    /// is that a hostile query param on the *detail-page* request (e.g. a
    /// forwarded `?ref=` campaign marker) must never be reflected into this
    /// href. Pure string composition: no DB/HTTP/env access. The `project`
    /// query param lets the (not-yet-built) `/signup` flow know which
    /// project prompted the signup, without ever touching anything the
    /// visitor's own request controlled.
    pub fn build_signup_deep_link(project_id: Uuid) -> String {
        format!("/signup?project={project_id}")
    }

    /// IMP-REQ-014-03: best-effort cross-site abuse reduction for
    /// `POST /api/v1/cta-events`. This is NOT a security boundary — both
    /// `Origin` and `Referer` are attacker-controlled headers on requests
    /// from outside a real browser, and a browser itself may omit both on
    /// some requests. When either header is present, its host must match
    /// `allowed_host` or the request is rejected; when BOTH are absent, the
    /// request is allowed through rather than rejected on a signal this
    /// check has no basis to evaluate (e.g. this repo's own integration
    /// tests exercise the route directly via `tower::ServiceExt::oneshot`
    /// with no simulated browser context, sending neither header).
    pub fn origin_check_passes(
        origin_header: Option<&str>,
        referer_header: Option<&str>,
        allowed_host: &str,
    ) -> bool {
        match origin_header.or(referer_header) {
            None => true,
            Some(value) => host_matches(value, allowed_host),
        }
    }

    fn host_matches(url_or_origin: &str, allowed_host: &str) -> bool {
        url::Url::parse(url_or_origin)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string))
            .map(|host| host.eq_ignore_ascii_case(allowed_host))
            .unwrap_or(false)
    }

    /// IMP-REQ-015-07: the project-detail page's own copy of
    /// `routes::search::core::format_detection_sentence` (this codebase's
    /// established convention for a small pure function needed identically
    /// in two route modules — see `locale.rs`'s own doc comment for when a
    /// genuinely shared module is warranted instead; this one-function
    /// duplication doesn't rise to that). Same contract exactly: `None`
    /// whenever either input is `None` or `source_count` is non-positive
    /// (TC-015-5), calendar-day difference, "today"/singular/plural EN+FR
    /// phrasing (TC-015-2/-3/-4/-6's search-card equivalents).
    pub fn format_detection_sentence(
        lang: &str,
        first_detected_at: Option<DateTime<Utc>>,
        source_count: Option<i64>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let first_detected_at = first_detected_at?;
        let source_count = source_count?;
        if source_count <= 0 {
            return None;
        }

        let days_ago = (now.date_naive() - first_detected_at.date_naive())
            .num_days()
            .max(0);
        let is_fr = lang == "fr";

        let days_part = match (is_fr, days_ago) {
            (true, 0) => "aujourd'hui".to_string(),
            (true, 1) => "il y a 1 jour".to_string(),
            (true, n) => format!("il y a {n} jours"),
            (false, 0) => "today".to_string(),
            (false, 1) => "1 day ago".to_string(),
            (false, n) => format!("{n} days ago"),
        };
        let source_part = match (is_fr, source_count) {
            (true, 1) => "1 source municipale".to_string(),
            (true, n) => format!("{n} sources municipales"),
            (false, 1) => "1 council source".to_string(),
            (false, n) => format!("{n} council sources"),
        };

        Some(if is_fr {
            format!("Détecté {days_part} depuis {source_part}")
        } else {
            format!("Detected {days_part} from {source_part}")
        })
    }

    /// IMP-REQ-014-01/-02: the small, fixed vocabulary of telemetry events
    /// the CTA card's own JS emits (mirrors the `cta_events.event_type`
    /// CHECK constraint in migration `026_cta_events.sql`) — validated here
    /// before ever reaching the DB, so an unrecognized event type is
    /// rejected with a clear `400` from the handler rather than surfacing a
    /// raw DB constraint-violation error.
    pub fn is_known_cta_event_type(event_type: &str) -> bool {
        matches!(event_type, "impression" | "click")
    }

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

        // -- canonical_url (IMP-REQ-013-06) --

        #[test]
        fn canonical_url_joins_base_and_project_path() {
            let id = uuid::Uuid::nil();
            assert_eq!(
                canonical_url("https://shovelsup.example", id),
                format!("https://shovelsup.example/projects/{id}")
            );
        }

        #[test]
        fn canonical_url_strips_trailing_slash_from_base() {
            let id = uuid::Uuid::nil();
            assert_eq!(
                canonical_url("https://shovelsup.example/", id),
                format!("https://shovelsup.example/projects/{id}")
            );
        }

        // -- build_signup_deep_link / origin_check_passes / is_known_cta_event_type (IMP-REQ-014-04/-03/-01) --

        #[test]
        fn build_signup_deep_link_reflects_only_the_validated_project_id() {
            let id = uuid::Uuid::nil();
            assert_eq!(
                build_signup_deep_link(id),
                format!("/signup?project={id}")
            );
        }

        #[test]
        fn format_detection_sentence_none_when_first_detected_at_missing() {
            let now = Utc::now();
            assert_eq!(format_detection_sentence("en", None, Some(2), now), None);
        }

        #[test]
        fn format_detection_sentence_none_when_source_count_missing() {
            let now = Utc::now();
            assert_eq!(
                format_detection_sentence("en", Some(now), None, now),
                None
            );
        }

        #[test]
        fn format_detection_sentence_none_when_source_count_non_positive() {
            let now = Utc::now();
            assert_eq!(format_detection_sentence("en", Some(now), Some(0), now), None);
        }

        #[test]
        fn format_detection_sentence_one_day_is_singular_not_plural() {
            let now = Utc::now();
            let one_day_ago = now - chrono::Duration::days(1);
            let sentence = format_detection_sentence("en", Some(one_day_ago), Some(1), now);
            assert_eq!(
                sentence,
                Some("Detected 1 day ago from 1 council source".to_string())
            );
        }

        #[test]
        fn format_detection_sentence_multiple_days_is_plural() {
            let now = Utc::now();
            let five_days_ago = now - chrono::Duration::days(5);
            let sentence = format_detection_sentence("en", Some(five_days_ago), Some(2), now);
            assert_eq!(
                sentence,
                Some("Detected 5 days ago from 2 council sources".to_string())
            );
        }

        #[test]
        fn format_detection_sentence_french_localization() {
            let now = Utc::now();
            let five_days_ago = now - chrono::Duration::days(5);
            let sentence = format_detection_sentence("fr", Some(five_days_ago), Some(2), now);
            assert_eq!(
                sentence,
                Some("Détecté il y a 5 jours depuis 2 sources municipales".to_string())
            );
        }

        #[test]
        fn origin_check_passes_when_both_headers_absent() {
            assert!(origin_check_passes(None, None, "shovelsup.example"));
        }

        #[test]
        fn origin_check_passes_when_origin_host_matches() {
            assert!(origin_check_passes(
                Some("https://shovelsup.example"),
                None,
                "shovelsup.example"
            ));
        }

        #[test]
        fn origin_check_fails_when_origin_host_does_not_match() {
            assert!(!origin_check_passes(
                Some("https://evil.example"),
                None,
                "shovelsup.example"
            ));
        }

        #[test]
        fn origin_check_falls_back_to_referer_when_origin_absent() {
            assert!(origin_check_passes(
                None,
                Some("https://shovelsup.example/projects/123"),
                "shovelsup.example"
            ));
        }

        #[test]
        fn origin_check_fails_when_referer_host_does_not_match() {
            assert!(!origin_check_passes(
                None,
                Some("https://evil.example/steal"),
                "shovelsup.example"
            ));
        }

        #[test]
        fn origin_check_fails_on_malformed_header_value() {
            assert!(!origin_check_passes(
                Some("not a url"),
                None,
                "shovelsup.example"
            ));
        }

        #[test]
        fn is_known_cta_event_type_accepts_impression_and_click() {
            assert!(is_known_cta_event_type("impression"));
            assert!(is_known_cta_event_type("click"));
        }

        #[test]
        fn is_known_cta_event_type_rejects_unknown_values() {
            assert!(!is_known_cta_event_type("bogus"));
            assert!(!is_known_cta_event_type(""));
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
/// REQ-013 (IMP-REQ-013-01/-04/-05/-06, done): `projects.merged_into_id`
/// (migration 025, self-referencing nullable FK + one-hop trigger) is
/// queried alongside `id`/`confidence_level` before any other lookup;
/// `get_project_detail_page` 301-redirects to `/projects/{merged_into_id}`
/// immediately when it is `Some`, never constructing `ProjectDetailContext`
/// or rendering this project's own page at all (TC-013-1) — so no
/// `merged_into_id` field lives on this struct itself (would always be
/// `None` here by construction). `canonical_url` is built by
/// `core::canonical_url` from the `PUBLIC_BASE_URL` env var (defaulting to
/// a production domain literal when unset — no such config existed before
/// this requirement), deliberately ignoring the request's own `Host`/
/// `X-Forwarded-Host` headers (TC-013-2), and rendered into
/// `project_detail.html` as both a `<link rel="canonical">` tag and
/// `og:url`/`og:title` Open Graph tags. Malformed (400) and nonexistent
/// (404) project ids both render the same friendly `not_found_labels`
/// bilingual copy through the existing `page_error` branch (TC-013-4/-5) —
/// no separate `project_not_found.html` template was introduced, since the
/// existing branch already extends `base.html` (viewport meta, stylesheet)
/// and needed only a label-source change, not new markup.
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
    // IMP-REQ-013-04/-06: `merged_into_id` is not carried into this struct
    // at all — by construction, `get_project_detail_page` never reaches
    // this struct's construction when the project has `merged_into_id` set
    // (it 301-redirects to the canonical project first, see TC-013-1), so
    // a field here would always be `None` and is redundant. `canonical_url`
    // IS populated for real, from `core::canonical_url` + a configured base
    // URL, and rendered as `project_detail.html`'s `<link rel="canonical">`
    // and Open Graph `og:url` tags (TC-013-2).
    canonical_url: Option<String>,
    // IMP-REQ-015-07/-12: "Detected N day(s) ago from M council source(s)",
    // via `core::format_detection_sentence` — `None` whenever either
    // underlying `public_search_documents` column is missing (TC-015-5).
    detection_sentence: Option<String>,
}

struct TimelineLabels {
    page_title: &'static str,
    timeline_title: &'static str,
    timeline_error_message: &'static str,
    retry_label: &'static str,
    timeline_empty_message: &'static str,
    status_update_fallback: &'static str,
    nav_projects: &'static str,
    confidence_notice_label: &'static str,
    confidence_unassessed_fallback: &'static str,
    source_document_link_label: &'static str,
    description_language_notice: &'static str,
    document_retrieved_fallback_label: &'static str,
    copy_link_label: &'static str,
    copy_link_copied_label: &'static str,
}

/// IMP-REQ-013-02: EN/FR labels for the friendly "project not found"-style
/// page, mirroring `search::search_labels`'s pattern (a plain lang-keyed
/// literal-struct function, no I/O). Deliberately shared by BOTH of
/// `get_project_detail_page`'s whole-page error branches — a malformed `:id`
/// path segment (400, TC-013-4) and a well-formed but nonexistent project id
/// (404, TC-013-5) — rather than each having its own distinct copy: both are
/// "we can't show you this project" from the visitor's point of view, and
/// TC-013-4 explicitly asserts the SAME friendly "Project not found"/"Projet
/// introuvable" copy for the 400 case as the 404 case, just as a different
/// HTTP status (a malformed id is still a client input error, distinct from
/// a well-formed-but-missing one — TC-013-4's doc comment). This replaces
/// the previous, narrower `TimelineLabels::not_found_title`/`bad_request_title`
/// fields (which gave the 400 case its own "Invalid request" copy) with one
/// unified label set.
struct NotFoundLabels {
    title: &'static str,
    message: &'static str,
}

fn not_found_labels(lang: &str) -> NotFoundLabels {
    match lang {
        "fr" => NotFoundLabels {
            title: "Projet introuvable",
            message: "Nous n’avons trouvé aucun projet correspondant à cet identifiant.",
        },
        _ => NotFoundLabels {
            title: "Project not found",
            message: "We couldn’t find a project matching that identifier.",
        },
    }
}

/// IMP-REQ-014-06: EN/FR copy for the project-detail page's non-modal
/// "Get alerts — sign up" upsell CTA card, mirroring `not_found_labels`'s
/// plain lang-keyed literal-struct pattern (no I/O).
struct CtaLabels {
    heading: &'static str,
    body: &'static str,
    signup_label: &'static str,
    collapse_label: &'static str,
    expand_label: &'static str,
}

fn cta_labels(lang: &str) -> CtaLabels {
    match lang {
        "fr" => CtaLabels {
            heading: "Recevez des alertes",
            body: "Inscrivez-vous pour recevoir des alertes sur les prochaines décisions du conseil concernant ce projet.",
            signup_label: "S’inscrire",
            collapse_label: "Réduire",
            expand_label: "Développer",
        },
        _ => CtaLabels {
            heading: "Get alerts",
            body: "Sign up to get alerts about future council decisions on this project.",
            signup_label: "Sign up",
            collapse_label: "Collapse",
            expand_label: "Expand",
        },
    }
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
            nav_projects: "Projets",
            confidence_notice_label: "Niveau de confiance : ",
            confidence_unassessed_fallback: "pas encore évalué",
            source_document_link_label: "Voir le document source",
            description_language_notice: "Cette description provient d’un document dans une autre langue que l’interface.",
            document_retrieved_fallback_label: "Document récupéré",
            copy_link_label: "Copier le lien",
            copy_link_copied_label: "Lien copié !",
        },
        _ => TimelineLabels {
            page_title: "Project details",
            timeline_title: "Project timeline",
            timeline_error_message: "We couldn’t load this project’s timeline.",
            retry_label: "Retry timeline",
            timeline_empty_message: "No timeline events have been recorded for this project yet.",
            status_update_fallback: "Update recorded",
            nav_projects: "Projects",
            confidence_notice_label: "Confidence level: ",
            confidence_unassessed_fallback: "not yet assessed",
            source_document_link_label: "View source document",
            description_language_notice: "This description is sourced from a document in a different language than this page.",
            document_retrieved_fallback_label: "Document retrieved",
            copy_link_label: "Copy link",
            copy_link_copied_label: "Copied!",
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
///
/// IMP-REQ-011-07: the `:id` path segment is extracted as a plain `String`
/// (not `Path<Uuid>`) and parsed manually below, rather than letting Axum's
/// `Uuid` extractor reject a malformed id itself. Axum's own extractor
/// rejection short-circuits BEFORE this handler body ever runs, returning a
/// bare `400` with an empty body — no viewport meta tag, no stylesheet link,
/// the exact "unstyled error" gap REQ-011 exists to close (TC-011-4's 400
/// case). Parsing here instead means a malformed id can render through
/// `project_detail.html`'s `page_error` branch like every other error case
/// on this page.
pub async fn get_project_detail_page(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    Query(params): Query<ProjectDetailParams>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
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

    // IMP-REQ-011-07: renders the DB-outage (503) timeline error, unchanged
    // from before this task (still passes TC-011-4's 503 case).
    let render_error_page = |labels: &TimelineLabels, id: Uuid| -> Result<Response, StatusCode> {
        let html = tmpl
            .render(context! {
                lang => lang,
                nav_projects => labels.nav_projects,
                page_title => labels.page_title,
                timeline_title => labels.timeline_title,
                timeline_error => true,
                timeline_error_message => labels.timeline_error_message,
                retry_label => labels.retry_label,
                timeline_retry_url => format!("/projects/{id}"),
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok((StatusCode::SERVICE_UNAVAILABLE, Html(html)).into_response())
    };

    // IMP-REQ-011-07: renders a whole-page error (404 "project not found" /
    // 400 "malformed id") through `project_detail.html`'s `page_error`
    // branch, so it still extends `base.html` and carries the same viewport
    // meta tag/stylesheet link as every normal page (TC-011-4's 404/400
    // cases) instead of a bare, unstyled `StatusCode` with an empty body.
    let render_page_error =
        |status: StatusCode, title: &str, message: &str| -> Result<Response, StatusCode> {
            let html = tmpl
                .render(context! {
                    lang => lang,
                    nav_projects => labels.nav_projects,
                    page_title => labels.page_title,
                    page_error => true,
                    page_error_title => title,
                    page_error_message => message,
                })
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok((status, Html(html)).into_response())
        };

    // IMP-REQ-013-02/-05: both the malformed-id (400) and nonexistent-id
    // (404) whole-page error branches below render the SAME friendly
    // "Project not found"/"Projet introuvable" copy (TC-013-4 asserts this
    // explicitly for the 400 case too) — only the HTTP status differs,
    // preserving the "malformed id is a client input error, distinct from a
    // well-formed-but-missing one" distinction TC-011-4/TC-013-4's doc
    // comments draw.
    let not_found = not_found_labels(lang);

    // IMP-REQ-011-07: a malformed `:id` path segment (not a well-formed
    // UUID) is a client error, not a 404 — same distinction TC-011-4's 400
    // vs. 404 sub-tests draw.
    let Ok(id) = raw_id.parse::<Uuid>() else {
        return render_page_error(StatusCode::BAD_REQUEST, not_found.title, not_found.message);
    };

    // IMP-REQ-013-04: a project that has been merged into another (canonical)
    // project must 301-redirect to that project's URL instead of rendering
    // its own (now superseded) page at all — checked before any other
    // lookup, exactly like TC-013-1 expects. `Location` is a relative path
    // (`/projects/{id}`); TC-013-1 asserts this exact relative form.
    // IMP-REQ-015-07: `first_detected_at`/`source_count` are read from
    // `public_search_documents` (LEFT JOIN, since a project may not yet be
    // materialized there — a genuinely missing row behaves identically to
    // an existing row with both columns `NULL`: the indicator omits itself
    // entirely, TC-015-5), the same denormalized columns the search-card
    // half (`SearchResult`, `routes/search.rs`) reads.
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

    if let Some(canonical_id) = project_row.merged_into_id {
        // TC-013-1 asserts a literal 301 (`StatusCode::MOVED_PERMANENTLY`),
        // not axum's `Redirect::permanent` helper (which emits 308
        // `PERMANENT_REDIRECT` — semantically similar but a different status
        // code, and not what the plan/test settled on), so the response is
        // built directly.
        let mut response = StatusCode::MOVED_PERMANENTLY.into_response();
        response.headers_mut().insert(
            axum::http::header::LOCATION,
            format!("/projects/{canonical_id}")
                .parse()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        return Ok(response);
    }

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

    // IMP-REQ-013-06: sourced from configuration (env var), never from this
    // request's own `Host`/`X-Forwarded-Host` header (TC-013-2) — a client
    // fully controls those headers, so trusting them to build a
    // security/SEO-sensitive canonical URL would let a spoofed `Host` poison
    // it. Defaults to the production domain when unset, so a deployment
    // that forgets to set `PUBLIC_BASE_URL` (or a test run, which never sets
    // it) still gets a valid, absolute `https://` canonical URL rather than
    // silently omitting the tag.
    let base_url = std::env::var("PUBLIC_BASE_URL")
        .unwrap_or_else(|_| "https://shovelsup.example".to_string());
    let canonical_url = core::canonical_url(&base_url, id);

    // IMP-REQ-014-05/-06/-07: this app has no auth at all (REQ-010), so
    // every `GET /projects/:id` view reaching this point is by construction
    // an anonymous visitor — the CTA card therefore renders unconditionally
    // whenever the page itself renders successfully (never omitted for an
    // "authenticated" branch, since no such branch exists in this app
    // today). `cta_signup_url` is built from the already-parsed, validated
    // `id` — never from `raw_id`/`params`/any request header — so no raw
    // request input can leak into the CTA's signup link (TC-014-3).
    let cta_labels = cta_labels(lang);
    let cta_signup_url = core::build_signup_deep_link(id);

    // IMP-REQ-015-07: computed here in the shell, where `lang` and a single
    // shared `now` are both known — mirrors `get_search_page`'s own
    // placement of this same derivation (see `routes/search.rs`).
    let detection_sentence = core::format_detection_sentence(
        lang,
        project_row.first_detected_at,
        project_row.source_count,
        Utc::now(),
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
        canonical_url: Some(canonical_url),
        detection_sentence,
    };

    let html = tmpl
        .render(context! {
            lang => lang,
            nav_projects => labels.nav_projects,
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
            canonical_url => detail_context.canonical_url,
            detection_sentence => detail_context.detection_sentence,
            copy_link_label => labels.copy_link_label,
            copy_link_copied_label => labels.copy_link_copied_label,
            project_id => id.to_string(),
            cta_heading => cta_labels.heading,
            cta_body => cta_labels.body,
            cta_signup_url => cta_signup_url,
            cta_signup_label => cta_labels.signup_label,
            cta_collapse_label => cta_labels.collapse_label,
            cta_expand_label => cta_labels.expand_label,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::OK, Html(html)).into_response())
}

/// Request body for `POST /api/v1/cta-events` (IMP-REQ-014-02): the CTA
/// card's own JS (IMP-REQ-014-09) fires one of these per fire-and-forget
/// `navigator.sendBeacon` call — `event` is validated against
/// `core::is_known_cta_event_type`'s fixed vocabulary before ever reaching
/// the DB.
#[derive(Debug, Deserialize)]
pub struct CtaEventRequest {
    pub project_id: Uuid,
    pub event: String,
}

/// POST /api/v1/cta-events (IMP-REQ-014-02/-03). Fully public, unauthenticated
/// endpoint (REQ-010's "no account required" guarantee — `public_router`
/// registers this route with no auth layer, same as every other route
/// there) that records one fire-and-forget telemetry beacon from the
/// project-detail page's non-modal CTA card. Returns `202 Accepted`: the
/// caller is `navigator.sendBeacon`/`fetch(..., {keepalive:true})`, which
/// never reads or acts on the response body — 202 ("accepted for
/// processing") is the correct semantic here, not `200`/`204`, which would
/// imply a stronger completion guarantee than "the write was attempted"
/// this handler actually gives once past the `?` on the `INSERT`.
///
/// IMP-REQ-014-03: `core::origin_check_passes` is a best-effort abuse
/// reduction signal, not a security boundary (see that function's doc
/// comment) — actual rate limiting is layered on this route at the router
/// level (`middleware::rate_limit::rate_limit_cta_events` in `lib.rs`), not
/// here.
///
/// TC-014-5: this route/handler is entirely independent of
/// `get_project_detail_page` — a failure here (origin-check rejection,
/// unknown event type, DB error) returns a 4xx/5xx from THIS handler alone
/// and never touches the detail page's own rendering, which is a
/// structurally separate Axum route registration.
pub async fn post_cta_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CtaEventRequest>,
) -> Result<StatusCode, StatusCode> {
    // IMP-REQ-014-03: the same `PUBLIC_BASE_URL` configuration
    // `core::canonical_url` uses (IMP-REQ-013-06) is the source of the
    // "expected" host for the origin/referer check — never the request's
    // own `Host`/`X-Forwarded-Host` header, for the same spoofability reason
    // TC-013-2 already established for the canonical URL.
    let base_url = std::env::var("PUBLIC_BASE_URL")
        .unwrap_or_else(|_| "https://shovelsup.example".to_string());
    let allowed_host = url::Url::parse(&base_url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| "shovelsup.example".to_string());

    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let referer = headers
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok());

    if !core::origin_check_passes(origin, referer, &allowed_host) {
        return Err(StatusCode::FORBIDDEN);
    }

    if !core::is_known_cta_event_type(&payload.event) {
        return Err(StatusCode::BAD_REQUEST);
    }

    sqlx::query!(
        "INSERT INTO cta_events (project_id, event_type) VALUES ($1, $2)",
        payload.project_id,
        payload.event,
    )
    .execute(&state.db)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(StatusCode::ACCEPTED)
}
