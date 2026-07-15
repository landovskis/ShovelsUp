use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::{routes::detect_lang, AppState};

const DEFAULT_PER_PAGE: i64 = 20;
const MAX_PER_PAGE: i64 = 100;

/// Pure, I/O-free validation of raw search query params (IMP-REQ-001-02).
/// Extracted out of `run_search`'s inline `per_page` bounds check so the
/// validation rule is independently unit-testable without a DB/HTTP server.
/// Wired into `run_search` (IMP-REQ-001-04).
mod core {
    use super::{DEFAULT_PER_PAGE, MAX_PER_PAGE};

    /// Search params that have passed validation and are ready to drive a
    /// query.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ValidatedSearchParams {
        pub per_page: i64,
    }

    /// Reasons `validate_search_params` can reject raw input.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SearchValidationError {
        /// `per_page`, after defaulting, fell outside `1..=MAX_PER_PAGE`.
        PerPageOutOfRange,
    }

    /// Validates raw search query params. Currently covers `per_page`
    /// (defaulting to `DEFAULT_PER_PAGE` when absent, rejecting anything
    /// outside `1..=MAX_PER_PAGE`); the plan does not call for `q`
    /// validation beyond what already exists (an empty `q` is a valid "no
    /// search yet" state handled by the caller), so this function stays
    /// scoped to `per_page` rather than growing speculative checks.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn validate_search_params(
        per_page: Option<i64>,
    ) -> Result<ValidatedSearchParams, SearchValidationError> {
        let per_page = per_page.unwrap_or(DEFAULT_PER_PAGE);
        if !(1..=MAX_PER_PAGE).contains(&per_page) {
            return Err(SearchValidationError::PerPageOutOfRange);
        }
        Ok(ValidatedSearchParams { per_page })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn defaults_to_default_per_page_when_absent() {
            let result = validate_search_params(None).unwrap();
            assert_eq!(result.per_page, DEFAULT_PER_PAGE);
        }

        #[test]
        fn accepts_a_valid_mid_range_value() {
            let result = validate_search_params(Some(50)).unwrap();
            assert_eq!(result.per_page, 50);
        }

        #[test]
        fn accepts_the_lower_bound() {
            let result = validate_search_params(Some(1)).unwrap();
            assert_eq!(result.per_page, 1);
        }

        #[test]
        fn accepts_the_upper_bound() {
            let result = validate_search_params(Some(MAX_PER_PAGE)).unwrap();
            assert_eq!(result.per_page, MAX_PER_PAGE);
        }

        #[test]
        fn rejects_zero() {
            let result = validate_search_params(Some(0));
            assert_eq!(result, Err(SearchValidationError::PerPageOutOfRange));
        }

        #[test]
        fn rejects_one_above_the_max() {
            let result = validate_search_params(Some(MAX_PER_PAGE + 1));
            assert_eq!(result, Err(SearchValidationError::PerPageOutOfRange));
        }

        #[test]
        fn rejects_negative_values() {
            let result = validate_search_params(Some(-1));
            assert_eq!(result, Err(SearchValidationError::PerPageOutOfRange));
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    pub per_page: Option<i64>,
    // Loop A stub: filtering logic added by IMP-REQ-002-04. Field exists so
    // TC-002-* tests in search_integration.rs compile against the future
    // `municipality_slug` query param; `run_search` does not read it yet,
    // and no validation against the `municipalities` table happens yet
    // (that's IMP-REQ-002-03/-04).
    pub municipality_slug: Option<String>,
    // Loop A stub: pagination wired by IMP-REQ-004-03/04. `run_search` does
    // not read this yet; TC-004-5 documents today's unpaginated-boundary gap.
    pub page: Option<i64>,
    // Loop A stub: DateFilter parsing/validation wired by IMP-REQ-007-03/05
    // against public_search_documents.first_surfaced_at once
    // IMP-REQ-004-01 lands.
    pub date_preset: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    // Loop A stub: category_taxonomy + validation wired by
    // IMP-REQ-008-02/03/04. Field exists so TC-008-* tests in
    // search_integration.rs compile against the future `category` query
    // param (e.g. `residential`, or the explicit `uncategorised`
    // pseudo-value for `category_code IS NULL`); `run_search` does not read
    // it yet, and no validation against the (not-yet-existing)
    // `category_taxonomy` table happens yet.
    pub category: Option<String>,
    // Loop A stub: sort param + latest_meeting_date ORDER BY wired by
    // IMP-REQ-009-04/06
    pub sort: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub project_id: uuid::Uuid,
    pub civic_address_normalized: String,
    pub municipality_name: Option<String>,
    pub project_type: Option<String>,
    pub normalized_status: Option<String>,
    // Loop A stub: populated by IMP-REQ-003-02/03/04 migration+backfill.
    pub source_language: Option<String>,
    // Loop A stub: synthesized display name (civic address + project type,
    // e.g. "Demolition — 123 Main St") added by IMP-REQ-004-09. There is no
    // `project_name` field to derive from yet, and `run_search` does not
    // populate this; TC-004-3 documents today's gap of it being absent.
    pub display_name: Option<String>,
    // Loop A stub: populated by IMP-REQ-015-02/03/04 migration+materializer.
    // `public_search_documents` has neither `first_detected_at` nor
    // `source_count` columns yet — that migration is this requirement's own
    // job, out of scope for this Loop A pass. Once both land, the
    // "Detected N days ago from M council source(s)" indicator
    // (IMP-REQ-015-06/-11) is derived from these two fields together and
    // omitted entirely when either is `None` (TC-015-5).
    pub first_detected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub source_count: Option<i64>,
}

/// Loop A stub: target-state paginated envelope for `GET
/// /api/v1/projects/search` (TC-004-1). `search_projects` still returns a
/// bare `Json<Vec<SearchResult>>` today — IMP-REQ-004-04 must change the
/// handler's return type to this envelope and populate `total`/`page`/
/// `per_page`/`has_more` from `run_search`'s (future) paginated query.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct SearchResultsEnvelope {
    pub results: Vec<SearchResult>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub has_more: bool,
}

/// Validates `per_page` (TC-REQ-008-3: rejected before any DB query runs)
/// and runs the keyword search shared by both the JSON API and the
/// server-rendered page. `q` matches against either the civic address or
/// the municipality name (TC-REQ-008-2: a query that only matches the
/// municipality, with no address-keyword overlap, still returns results).
async fn run_search(
    pool: &sqlx::PgPool,
    q: &str,
    per_page: Option<i64>,
) -> Result<Vec<SearchResult>, StatusCode> {
    let core::ValidatedSearchParams { per_page } = core::validate_search_params(per_page)
        .map_err(|core::SearchValidationError::PerPageOutOfRange| StatusCode::BAD_REQUEST)?;

    let keyword = format!("%{q}%");
    let rows = sqlx::query!(
        r#"
        SELECT project_id, civic_address_normalized, municipality_name, project_type, normalized_status
        FROM public_search_documents
        WHERE civic_address_normalized ILIKE $1 OR municipality_name ILIKE $1
        ORDER BY civic_address_normalized ASC
        LIMIT $2
        "#,
        keyword,
        per_page
    )
    .fetch_all(pool)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .into_iter()
    .map(|row| SearchResult {
        project_id: row.project_id,
        civic_address_normalized: row.civic_address_normalized,
        municipality_name: row.municipality_name,
        project_type: row.project_type,
        normalized_status: row.normalized_status,
        // Loop A stub: `public_search_documents.source_language` doesn't
        // exist yet; IMP-REQ-003-02 (migration) + IMP-REQ-003-04 (route
        // wiring) must select and set the real value here.
        source_language: None,
        // Loop A stub: see `SearchResult::display_name` doc comment.
        display_name: None,
        // Loop A stub: see `SearchResult::first_detected_at`/`source_count`
        // doc comment — IMP-REQ-015-02/03/04 migration+materializer.
        first_detected_at: None,
        source_count: None,
    })
    .collect();

    Ok(rows)
}

/// GET /api/v1/projects/search — public, unauthenticated keyword search
/// (TC-REQ-008-1..4).
pub async fn search_projects(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<SearchResult>>, StatusCode> {
    let results = run_search(&state.db, &params.q, params.per_page).await?;
    Ok(Json(results))
}

/// Loop A stub: target-state category facet endpoint (`GET /categories`,
/// TC-008-4/-5). Not yet wired into the router in `web/src/lib.rs` — that's
/// IMP-REQ-008-04's job, once the `category_taxonomy` table exists
/// (IMP-REQ-008-02) for it to query. Today it unconditionally returns 501
/// regardless of DB state; IMP-REQ-008-04/-13 must replace this with a real
/// `State<AppState>`-taking handler that queries `category_taxonomy` and
/// degrades gracefully (e.g. 503) if that query fails, per TC-008-5.
pub async fn list_categories() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

struct SearchLabels {
    page_title: &'static str,
    heading: &'static str,
    search_label: &'static str,
    submit_label: &'static str,
    empty_message: &'static str,
    // IMP-REQ-001-06: one extra guidance line shown alongside
    // `empty_message` in the zero-results state, suggesting the user
    // broaden/adjust their query. Deliberately a single, narrowly-scoped
    // addition — NOT the richer headline/body/suggestions/action-links
    // empty-state redesign that REQ-012 will add later to this same
    // template area (see `search-empty-guidance` in search.html, kept
    // distinct from REQ-012's future `search-empty-heading` /
    // `search-empty-body` / `search-empty-suggestion` /
    // `search-empty-action` element classes).
    empty_guidance: &'static str,
    nav_permits: &'static str,
    nav_council: &'static str,
}

fn search_labels(lang: &str) -> SearchLabels {
    match lang {
        "fr" => SearchLabels {
            page_title: "Recherche de projets",
            heading: "Rechercher un projet",
            search_label: "Adresse civique ou municipalité",
            submit_label: "Rechercher",
            empty_message: "Aucun projet ne correspond à votre recherche.",
            empty_guidance: "Essayez une recherche plus large : utilisez un mot-clé plus général ou vérifiez l'orthographe de l'adresse ou de la municipalité.",
            nav_permits: "Permis",
            nav_council: "Conseil",
        },
        _ => SearchLabels {
            page_title: "Search projects",
            heading: "Search for a project",
            search_label: "Civic address or municipality",
            submit_label: "Search",
            empty_message: "No projects match your search.",
            empty_guidance: "Try broadening your search: use a more general keyword, or double-check the spelling of the address or municipality.",
            nav_permits: "Permits",
            nav_council: "Council",
        },
    }
}

/// GET /search — server-rendered public search page (IMP-REQ-008-04),
/// EN/FR via `Accept-Language` matching the rest of the app's convention.
/// With no `q` param (first page load), renders the bare form. With `q`
/// present, runs the search server-side and renders results/empty/error
/// inline — no client-side JS round trip to the JSON API, avoiding a
/// mismatch between that endpoint's JSON body and this page's HTML.
pub async fn get_search_page(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
    headers: HeaderMap,
) -> Result<Html<String>, StatusCode> {
    let lang = detect_lang(&headers);
    let labels = search_labels(lang);

    let tmpl = state
        .env
        .get_template("search.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let has_searched = !params.q.is_empty();
    let search_outcome = if has_searched {
        Some(run_search(&state.db, &params.q, params.per_page).await)
    } else {
        None
    };

    let (search_results, search_error) = match search_outcome {
        Some(Ok(results)) => (results, false),
        Some(Err(_)) => (Vec::new(), true),
        None => (Vec::new(), false),
    };

    let html = tmpl
        .render(context! {
            lang => lang,
            nav_permits => labels.nav_permits,
            nav_council => labels.nav_council,
            page_title => labels.page_title,
            heading => labels.heading,
            search_label => labels.search_label,
            submit_label => labels.submit_label,
            empty_message => labels.empty_message,
            empty_guidance => labels.empty_guidance,
            query => params.q,
            has_searched => has_searched,
            search_results => search_results,
            search_error => search_error,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}
