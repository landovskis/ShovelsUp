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
        /// `municipality_slug`, after lowercasing, contained characters
        /// outside `[a-z0-9-]`, or was longer than
        /// `MAX_MUNICIPALITY_SLUG_LEN`.
        InvalidMunicipalitySlugFormat,
    }

    /// Generous upper bound on a syntactically valid slug's length. Real
    /// municipality slugs (`montreal`, `toronto`, `vancouver`, ...) are a
    /// handful of characters; this just guards against pathological input
    /// before it ever reaches a query, not a precise business rule.
    const MAX_MUNICIPALITY_SLUG_LEN: usize = 100;

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

    /// Pure, syntactic-only validation of a raw `municipality_slug` query
    /// param (IMP-REQ-002-03). Deliberately does NOT check whether the slug
    /// exists in the `municipalities` table — that's a DB-touching concern
    /// layered on top by the handler wiring (IMP-REQ-002-04), which queries
    /// the live table rather than a hardcoded list.
    ///
    /// - `None` or an empty/whitespace-only string means "no filter
    ///   applied", so it returns `Ok(None)`.
    /// - Otherwise the value is trimmed and lowercased (TC-002-6: a
    ///   mixed-case slug like `MONTREAL` is normalized and accepted, not
    ///   rejected — slugs are rendered lowercase by the server's own
    ///   `<select>` markup, so case seen server-side signals a client
    ///   normalization quirk, not an implausible lookup).
    /// - After normalization, anything other than lowercase ASCII
    ///   alphanumerics and hyphens, or a length beyond
    ///   `MAX_MUNICIPALITY_SLUG_LEN`, is rejected.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    ///
    /// Wired into `run_search` (IMP-REQ-002-04), which layers the live-table
    /// existence check on top.
    pub fn validate_municipality_slug(
        raw: Option<String>,
    ) -> Result<Option<String>, SearchValidationError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let normalized = trimmed.to_lowercase();
        let is_valid_shape = normalized.len() <= MAX_MUNICIPALITY_SLUG_LEN
            && normalized
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !is_valid_shape {
            return Err(SearchValidationError::InvalidMunicipalitySlugFormat);
        }

        Ok(Some(normalized))
    }

    /// Maps a normalized municipality slug (as produced by
    /// `validate_municipality_slug`) to its localized display name
    /// (IMP-REQ-002-05). Covers the launch set of three municipalities:
    /// Montreal has a distinct French form ("Montréal"); Toronto and
    /// Vancouver do not, so both languages share the same spelling.
    ///
    /// Returns `None` for a slug outside the launch set rather than
    /// panicking or guessing at a display name — the plan notes the launch
    /// set could grow, so an unrecognized slug is a caller-visible "I don't
    /// know this one yet" rather than a hardcoded failure.
    ///
    /// Expects `slug` to already be lowercase-normalized (as
    /// `validate_municipality_slug` does upstream); this function does not
    /// itself lowercase or trim, so a mixed-case or unnormalized slug will
    /// simply fail to match and return `None`.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn municipality_display_name(slug: &str, lang: &str) -> Option<&'static str> {
        match (slug, lang) {
            ("montreal", "fr") => Some("Montréal"),
            ("montreal", _) => Some("Montreal"),
            ("toronto", _) => Some("Toronto"),
            ("vancouver", _) => Some("Vancouver"),
            _ => None,
        }
    }

    /// Builds the "N results found" header shown above a non-empty result
    /// list (IMP-REQ-001-08), in EN or FR, with correct singular/plural
    /// wording. Pure string formatting from an already-known count — no
    /// I/O — so it lives in `core` alongside `validate_search_params` and is
    /// unit-testable without a DB/HTTP server. `get_search_page` only calls
    /// this when there's at least one result; the zero-results case is
    /// handled entirely by the existing `empty_message`/`empty_guidance`
    /// pair (IMP-REQ-001-06) so the two don't double up.
    pub fn format_result_count_label(lang: &str, count: usize) -> String {
        match (lang, count) {
            ("fr", 1) => "1 résultat trouvé".to_string(),
            ("fr", n) => format!("{n} résultats trouvés"),
            (_, 1) => "1 result found".to_string(),
            (_, n) => format!("{n} results found"),
        }
    }

    /// Builds the municipality-specific zero-results message (IMP-REQ-002-08),
    /// shown in place of the generic `empty_message` (IMP-REQ-001-06) when a
    /// search was scoped to a specific municipality and returned no matches.
    /// `municipality_display_name` is the already-localized name (e.g.
    /// "Montréal" in `fr`), so this function only interpolates it into a
    /// language-appropriate sentence — it does not itself resolve or
    /// localize the name.
    ///
    /// Pure string formatting — no I/O — so it lives in `core` alongside
    /// `format_result_count_label` and is unit-testable without a DB/HTTP
    /// server.
    pub fn format_municipality_empty_message(lang: &str, municipality_display_name: &str) -> String {
        match lang {
            "fr" => format!(
                "Aucun projet trouvé à {municipality_display_name} correspondant à votre recherche."
            ),
            _ => format!(
                "No projects found in {municipality_display_name} matching your search."
            ),
        }
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

        #[test]
        fn formats_singular_english() {
            assert_eq!(format_result_count_label("en", 1), "1 result found");
        }

        #[test]
        fn formats_plural_english() {
            assert_eq!(format_result_count_label("en", 3), "3 results found");
        }

        #[test]
        fn formats_singular_french() {
            assert_eq!(
                format_result_count_label("fr", 1),
                "1 résultat trouvé"
            );
        }

        #[test]
        fn formats_plural_french() {
            assert_eq!(
                format_result_count_label("fr", 3),
                "3 résultats trouvés"
            );
        }

        /// Documents `format_result_count_label`'s actual behavior at
        /// `count = 0`. `get_search_page` never calls this function with 0
        /// today (the call site is gated on `!search_results.is_empty()`),
        /// but the function is a public part of `core`'s contract and its
        /// match arms have no special-case for zero, so it falls into the
        /// plural branch in both languages ("0 results found" /
        /// "0 résultats trouvés") rather than returning `None` or an empty
        /// string. This test pins that behavior; it does not assert it is
        /// the "right" UX for a hypothetical future caller.
        #[test]
        fn formats_zero_as_plural_in_both_languages() {
            assert_eq!(format_result_count_label("en", 0), "0 results found");
            assert_eq!(
                format_result_count_label("fr", 0),
                "0 résultats trouvés"
            );
        }

        #[test]
        fn municipality_slug_none_input_means_no_filter() {
            let result = validate_municipality_slug(None).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn municipality_slug_empty_string_means_no_filter() {
            let result = validate_municipality_slug(Some(String::new())).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn municipality_slug_valid_lowercase_is_accepted_unchanged() {
            let result = validate_municipality_slug(Some("montreal".to_string())).unwrap();
            assert_eq!(result, Some("montreal".to_string()));
        }

        #[test]
        fn municipality_slug_with_hyphens_and_digits_is_accepted() {
            let result =
                validate_municipality_slug(Some("saint-jean-2".to_string())).unwrap();
            assert_eq!(result, Some("saint-jean-2".to_string()));
        }

        /// TC-002-6's committed behavior: uppercase/mixed-case input is
        /// normalized to lowercase and accepted, not rejected.
        #[test]
        fn municipality_slug_uppercase_is_normalized_to_lowercase() {
            let result = validate_municipality_slug(Some("MONTREAL".to_string())).unwrap();
            assert_eq!(result, Some("montreal".to_string()));

            let result = validate_municipality_slug(Some("Montreal".to_string())).unwrap();
            assert_eq!(result, Some("montreal".to_string()));
        }

        #[test]
        fn municipality_slug_with_invalid_characters_is_rejected() {
            let result = validate_municipality_slug(Some("mont real!".to_string()));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidMunicipalitySlugFormat)
            );

            let result = validate_municipality_slug(Some("montreal_qc".to_string()));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidMunicipalitySlugFormat)
            );

            let result = validate_municipality_slug(Some("montréal".to_string()));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidMunicipalitySlugFormat)
            );
        }

        /// Whitespace-only input is treated the same as an empty string
        /// (trimmed to nothing) rather than rejected: a caller sending
        /// `municipality_slug=%20` almost certainly means "no filter", the
        /// same as omitting the param entirely, not an invalid value.
        #[test]
        fn municipality_slug_whitespace_only_is_trimmed_to_no_filter() {
            let result = validate_municipality_slug(Some("   ".to_string())).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn municipality_slug_surrounding_whitespace_is_trimmed() {
            let result = validate_municipality_slug(Some("  montreal  ".to_string())).unwrap();
            assert_eq!(result, Some("montreal".to_string()));
        }

        #[test]
        fn municipality_slug_too_long_is_rejected() {
            let too_long = "a".repeat(MAX_MUNICIPALITY_SLUG_LEN + 1);
            let result = validate_municipality_slug(Some(too_long));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidMunicipalitySlugFormat)
            );
        }

        #[test]
        fn municipality_slug_at_max_length_is_accepted() {
            let max_len = "a".repeat(MAX_MUNICIPALITY_SLUG_LEN);
            let result = validate_municipality_slug(Some(max_len.clone())).unwrap();
            assert_eq!(result, Some(max_len));
        }

        #[test]
        fn municipality_display_name_montreal_english() {
            assert_eq!(
                municipality_display_name("montreal", "en"),
                Some("Montreal")
            );
        }

        #[test]
        fn municipality_display_name_montreal_french() {
            assert_eq!(
                municipality_display_name("montreal", "fr"),
                Some("Montréal")
            );
        }

        #[test]
        fn municipality_display_name_toronto_english() {
            assert_eq!(
                municipality_display_name("toronto", "en"),
                Some("Toronto")
            );
        }

        /// Toronto has no distinct French form: both languages share the
        /// same spelling.
        #[test]
        fn municipality_display_name_toronto_french() {
            assert_eq!(
                municipality_display_name("toronto", "fr"),
                Some("Toronto")
            );
        }

        #[test]
        fn municipality_display_name_vancouver_english() {
            assert_eq!(
                municipality_display_name("vancouver", "en"),
                Some("Vancouver")
            );
        }

        /// Vancouver has no distinct French form: both languages share the
        /// same spelling.
        #[test]
        fn municipality_display_name_vancouver_french() {
            assert_eq!(
                municipality_display_name("vancouver", "fr"),
                Some("Vancouver")
            );
        }

        #[test]
        fn municipality_display_name_unrecognized_slug_returns_none() {
            assert_eq!(municipality_display_name("gotham", "en"), None);
            assert_eq!(municipality_display_name("gotham", "fr"), None);
        }

        /// Documents the lowercase-normalization contract: this function
        /// does not itself normalize input, so a slug that hasn't already
        /// been through `validate_municipality_slug` (e.g. still mixed-case)
        /// simply fails to match and returns `None` rather than being
        /// case-folded internally.
        #[test]
        fn municipality_display_name_requires_lowercase_normalized_input() {
            assert_eq!(municipality_display_name("Montreal", "en"), None);
            assert_eq!(municipality_display_name("MONTREAL", "fr"), None);
        }

        #[test]
        fn format_municipality_empty_message_english() {
            assert_eq!(
                format_municipality_empty_message("en", "Montreal"),
                "No projects found in Montreal matching your search."
            );
        }

        #[test]
        fn format_municipality_empty_message_french() {
            assert_eq!(
                format_municipality_empty_message("fr", "Montréal"),
                "Aucun projet trouvé à Montréal correspondant à votre recherche."
            );
        }

        /// Any language other than `"fr"` falls back to the English wording,
        /// matching the same convention as `format_result_count_label`'s
        /// `(_, n)` match arm.
        #[test]
        fn format_municipality_empty_message_unknown_lang_falls_back_to_english() {
            assert_eq!(
                format_municipality_empty_message("de", "Vancouver"),
                "No projects found in Vancouver matching your search."
            );
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    pub per_page: Option<i64>,
    // IMP-REQ-002-04: syntactically validated via `core::validate_municipality_slug`,
    // then checked against the live `municipalities` table, then applied as
    // an `AND municipality_slug = $N` filter in `run_search`.
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

/// A single `<option>` in the search form's municipality `<select>`
/// (IMP-REQ-002-06). `slug` is the value submitted as `municipality_slug`;
/// `display_name` is the localized label shown to the user, resolved via
/// `core::municipality_display_name` (falling back to the raw DB `name` for
/// any municipality outside today's launch set, so an unrecognized future
/// municipality still renders something sensible instead of an empty
/// option).
#[derive(Debug, Serialize)]
pub struct MunicipalityOption {
    pub slug: String,
    pub display_name: String,
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
    municipality_slug: Option<String>,
) -> Result<Vec<SearchResult>, StatusCode> {
    let core::ValidatedSearchParams { per_page } =
        core::validate_search_params(per_page).map_err(|_| StatusCode::BAD_REQUEST)?;

    // IMP-REQ-002-04: syntactic validation first (TC-002-2/-6), before any
    // DB query runs.
    let municipality_slug = core::validate_municipality_slug(municipality_slug)
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // If a slug was supplied and is syntactically valid, it must also exist
    // in the live `municipalities` table (TC-002-2: a slug that is
    // well-formed but doesn't exist is still rejected with 400, not silently
    // treated as "no matches").
    if let Some(slug) = &municipality_slug {
        let exists = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM municipalities WHERE slug = $1)",
            slug
        )
        .fetch_one(pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .unwrap_or(false);

        if !exists {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let keyword = format!("%{q}%");
    let rows = sqlx::query!(
        r#"
        SELECT project_id, civic_address_normalized, municipality_name, project_type, normalized_status
        FROM public_search_documents
        WHERE (civic_address_normalized ILIKE $1 OR municipality_name ILIKE $1)
          AND ($3::text IS NULL OR municipality_slug = $3)
        ORDER BY civic_address_normalized ASC
        LIMIT $2
        "#,
        keyword,
        per_page,
        municipality_slug
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
    let results = run_search(
        &state.db,
        &params.q,
        params.per_page,
        params.municipality_slug,
    )
    .await?;
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
    municipality_select_label: &'static str,
    municipality_all_option: &'static str,
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
            municipality_select_label: "Municipalité",
            municipality_all_option: "Toutes les municipalités",
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
            municipality_select_label: "Municipality",
            municipality_all_option: "All municipalities",
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
        Some(
            run_search(
                &state.db,
                &params.q,
                params.per_page,
                params.municipality_slug.clone(),
            )
            .await,
        )
    } else {
        None
    };

    let (search_results, search_error) = match search_outcome {
        Some(Ok(results)) => (results, false),
        Some(Err(_)) => (Vec::new(), true),
        None => (Vec::new(), false),
    };

    // IMP-REQ-002-06: populates the search form's municipality `<select>`
    // from the live `municipalities` table (not a hardcoded list, same
    // principle as the `municipality_slug` backend validation in
    // `run_search`). A query failure here degrades to an empty options list
    // (select still renders, just with only the blank "no filter" option)
    // rather than failing the whole page render.
    let municipalities: Vec<MunicipalityOption> = sqlx::query!(
        "SELECT slug, name FROM municipalities ORDER BY slug ASC"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|row| MunicipalityOption {
        display_name: core::municipality_display_name(&row.slug, lang)
            .map(str::to_string)
            .unwrap_or(row.name),
        slug: row.slug,
    })
    .collect();

    // IMP-REQ-001-08: only shown alongside a non-empty result list — the
    // zero-results case stays exclusively owned by
    // `empty_message`/`empty_guidance` (IMP-REQ-001-06) so the two never
    // double up.
    let result_count_label = if !search_error && !search_results.is_empty() {
        Some(core::format_result_count_label(lang, search_results.len()))
    } else {
        None
    };

    // IMP-REQ-002-08: when the search was scoped to a specific, real
    // municipality (syntactically valid slug that also matched a row in
    // `municipalities` — otherwise `run_search` would have rejected it with
    // `search_error = true` before this point) and came back with zero
    // matches, replace the generic `empty_message` with one naming that
    // municipality. Looked up from the already-fetched `municipalities` list
    // (rather than re-deriving from `core::municipality_display_name`
    // directly) so the message uses the exact same localized display name
    // already shown as "selected" in the `<select>` control, including the
    // DB-`name` fallback for any municipality outside the hardcoded launch
    // set.
    let municipality_empty_message = if has_searched && !search_error && search_results.is_empty() {
        core::validate_municipality_slug(params.municipality_slug.clone())
            .ok()
            .flatten()
            .and_then(|slug| municipalities.iter().find(|m| m.slug == slug))
            .map(|m| core::format_municipality_empty_message(lang, &m.display_name))
    } else {
        None
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
            municipality_empty_message => municipality_empty_message,
            municipality_select_label => labels.municipality_select_label,
            municipality_all_option => labels.municipality_all_option,
            query => params.q,
            has_searched => has_searched,
            search_results => search_results,
            search_error => search_error,
            result_count_label => result_count_label,
            municipalities => municipalities,
            selected_municipality_slug => params.municipality_slug,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}
