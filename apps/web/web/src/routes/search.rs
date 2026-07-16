use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::AppState;

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

    /// Normalizes a raw `q` search query string for FTS matching
    /// (IMP-REQ-003-03). Trims outer whitespace and collapses any run of
    /// internal whitespace (spaces, tabs, newlines) down to a single space.
    ///
    /// Deliberately does NOT escape or wrap the text in `to_tsquery`
    /// operator syntax (`&`, `|`, `!`, `:*`, parentheses): this is intended
    /// to feed Postgres's `plainto_tsquery`, which already treats its input
    /// as plain free-text — it tokenizes on whitespace/punctuation itself
    /// and has no operator syntax for a caller to accidentally trigger or
    /// need escaped, unlike `to_tsquery`. Wrapping raw user text for
    /// `to_tsquery` instead would require actual escaping of its operator
    /// characters, which is a materially bigger job than this task's "plain
    /// keyword search" scope calls for; if the plan later needs
    /// `to_tsquery`'s richer operators, that's a separate follow-up, not a
    /// speculative addition here.
    ///
    /// Not yet wired into `run_search` (IMP-REQ-003-04's job).
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn normalize_query(q: &str) -> String {
        q.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Parses a single lang value (an explicit `?lang=` param or a `lang`
    /// cookie value, already extracted from the `Cookie` header by the
    /// caller) into a recognized `"fr"`/`"en"` tag. Case-insensitive on the
    /// exact two-letter code; anything else (garbage like `"xx"`, empty
    /// string, a full BCP-47 tag) is treated as unrecognized rather than
    /// guessed at, so the caller falls through to the next precedence
    /// level instead of misinterpreting it.
    fn parse_lang_tag(raw: &str) -> Option<&'static str> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("fr") {
            Some("fr")
        } else if trimmed.eq_ignore_ascii_case("en") {
            Some("en")
        } else {
            None
        }
    }

    /// Parses a raw `Accept-Language` header value the same way
    /// `routes::detect_lang` does (first comma-separated tag, ignoring
    /// `;q=` weights, prefix-matched against `fr`/`en`), but as a pure
    /// function over an already-extracted `&str` rather than a `HeaderMap`,
    /// so it can be unit-tested and reused from `resolve_ui_locale` without
    /// an HTTP request in hand. Defaults to `"en"` when nothing recognized
    /// is found, matching `detect_lang`'s existing behavior.
    fn parse_accept_language(raw: &str) -> &'static str {
        for part in raw.split(',') {
            let tag = part.split(';').next().unwrap_or("").trim();
            if tag.starts_with("fr") {
                return "fr";
            }
            if tag.starts_with("en") {
                return "en";
            }
        }
        "en"
    }

    /// Resolves the page's UI rendering locale (IMP-REQ-003-03) from the
    /// three sources REQ-003 commits to, in precedence order: an explicit
    /// `?lang=` query param, then a `lang` cookie, then the
    /// `Accept-Language` header, then `"en"` as the final default.
    ///
    /// Each source is tried in turn via `parse_lang_tag`
    /// (`explicit_param`/`cookie`) or `parse_accept_language`
    /// (`accept_language_header`); a source that is absent (`None`) or
    /// present but unrecognized (e.g. an `xx` cookie) does not short-circuit
    /// to the default — it simply falls through to the next source in the
    /// precedence chain, so a garbage explicit param still lets a valid
    /// cookie or header win, and only exhausting all three falls back to
    /// `"en"`.
    ///
    /// Callers pass already-extracted values: `cookie` is the `lang` cookie's
    /// value (not the raw `Cookie` header), and `accept_language_header` is
    /// the raw `Accept-Language` header value (not a parsed tag) since it
    /// needs `parse_accept_language`'s comma-list handling internally.
    ///
    /// Not yet wired into `get_search_page` (IMP-REQ-003-04's job, which
    /// must also add the `?lang=` param and `Cookie` header extraction this
    /// function depends on).
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn resolve_ui_locale(
        explicit_param: Option<&str>,
        cookie: Option<&str>,
        accept_language_header: Option<&str>,
    ) -> &'static str {
        if let Some(lang) = explicit_param.and_then(parse_lang_tag) {
            return lang;
        }
        if let Some(lang) = cookie.and_then(parse_lang_tag) {
            return lang;
        }
        if let Some(header) = accept_language_header {
            return parse_accept_language(header);
        }
        "en"
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

        #[test]
        fn normalize_query_empty_string_stays_empty() {
            assert_eq!(normalize_query(""), "");
        }

        #[test]
        fn normalize_query_whitespace_only_becomes_empty() {
            assert_eq!(normalize_query("   \t\n  "), "");
        }

        #[test]
        fn normalize_query_trims_surrounding_whitespace() {
            assert_eq!(normalize_query("  saint-denis  "), "saint-denis");
        }

        #[test]
        fn normalize_query_collapses_internal_whitespace_runs() {
            assert_eq!(normalize_query("rue   saint-denis"), "rue saint-denis");
            assert_eq!(
                normalize_query("rue\tsaint-denis\n\nmontreal"),
                "rue saint-denis montreal"
            );
        }

        #[test]
        fn normalize_query_leaves_ordinary_text_unchanged() {
            assert_eq!(normalize_query("demolition permit"), "demolition permit");
        }

        /// `normalize_query` intentionally does not escape or strip
        /// characters that are operator syntax for `to_tsquery` (`&`, `|`,
        /// `!`, `:`, parentheses) — it feeds `plainto_tsquery`, which has no
        /// such operator syntax to escape. This pins that the characters
        /// pass through untouched (beyond whitespace collapsing).
        #[test]
        fn normalize_query_leaves_special_characters_untouched() {
            assert_eq!(
                normalize_query("rock & roll (demolition!)"),
                "rock & roll (demolition!)"
            );
            assert_eq!(normalize_query("permit:2024"), "permit:2024");
        }

        #[test]
        fn resolve_ui_locale_explicit_param_wins_over_cookie_and_header() {
            assert_eq!(
                resolve_ui_locale(Some("fr"), Some("en"), Some("en")),
                "fr"
            );
        }

        #[test]
        fn resolve_ui_locale_cookie_wins_over_header_when_no_explicit_param() {
            assert_eq!(resolve_ui_locale(None, Some("fr"), Some("en")), "fr");
        }

        #[test]
        fn resolve_ui_locale_falls_back_to_accept_language_header_alone() {
            assert_eq!(resolve_ui_locale(None, None, Some("fr")), "fr");
            assert_eq!(resolve_ui_locale(None, None, Some("en")), "en");
        }

        #[test]
        fn resolve_ui_locale_defaults_to_english_when_nothing_present() {
            assert_eq!(resolve_ui_locale(None, None, None), "en");
        }

        /// An invalid/garbage explicit param (e.g. `xx`) does not win by
        /// virtue of being present — it's unrecognized, so resolution falls
        /// through to the cookie.
        #[test]
        fn resolve_ui_locale_invalid_explicit_param_falls_through_to_cookie() {
            assert_eq!(
                resolve_ui_locale(Some("xx"), Some("fr"), Some("en")),
                "fr"
            );
        }

        /// An invalid/garbage cookie value falls through to the
        /// `Accept-Language` header rather than being treated as a match or
        /// panicking.
        #[test]
        fn resolve_ui_locale_invalid_cookie_falls_through_to_header() {
            assert_eq!(resolve_ui_locale(None, Some("xx"), Some("fr")), "fr");
        }

        /// When every source is either absent or unrecognized, resolution
        /// still falls back to the `"en"` default rather than panicking.
        #[test]
        fn resolve_ui_locale_all_invalid_or_absent_defaults_to_english() {
            assert_eq!(resolve_ui_locale(Some("xx"), Some("zz"), None), "en");
            assert_eq!(resolve_ui_locale(Some(""), Some(""), Some("")), "en");
        }

        #[test]
        fn resolve_ui_locale_explicit_param_is_case_insensitive() {
            assert_eq!(resolve_ui_locale(Some("FR"), None, None), "fr");
            assert_eq!(resolve_ui_locale(Some("En"), None, None), "en");
        }

        #[test]
        fn resolve_ui_locale_accept_language_header_uses_first_recognized_tag_with_quality_weights() {
            assert_eq!(
                resolve_ui_locale(None, None, Some("fr;q=0.8,en;q=0.5")),
                "fr"
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
    // IMP-REQ-003-04: explicit UI-locale override, highest-precedence input
    // to `core::resolve_ui_locale`.
    pub lang: Option<String>,
}

/// Extracts the `lang` cookie's value from a raw `Cookie` request header
/// (IMP-REQ-003-04). `Cookie` headers are a single `; `-separated list of
/// `name=value` pairs (RFC 6265 §5.4) — this is a minimal parser scoped to
/// finding one specific cookie by name, not a general cookie-jar
/// implementation, since that's all `resolve_ui_locale` needs.
fn extract_cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim())
    })
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
    // IMP-REQ-003-04: `search_vector_fr`/`search_vector_en` (migration 018)
    // give FTS-stemmed matching alongside the existing ILIKE substring
    // match, so a French-stemmed query (e.g. "démolition") can match a
    // document containing a different inflected form ("démolir") the way
    // plain ILIKE cannot (TC-003-1). Matched against BOTH language vectors
    // regardless of the resolved UI locale — a French speaker's UI can
    // still find an English-sourced document and vice versa; `q`'s
    // language, not the page's rendering language, decides the match.
    let normalized_query = core::normalize_query(q);
    let rows = sqlx::query!(
        r#"
        SELECT project_id, civic_address_normalized, municipality_name, project_type, normalized_status, source_language
        FROM public_search_documents
        WHERE (
            civic_address_normalized ILIKE $1
            OR municipality_name ILIKE $1
            OR search_vector_fr @@ plainto_tsquery('french', $4)
            OR search_vector_en @@ plainto_tsquery('english', $4)
        )
          AND ($3::text IS NULL OR municipality_slug = $3)
        ORDER BY civic_address_normalized ASC
        LIMIT $2
        "#,
        keyword,
        per_page,
        municipality_slug,
        normalized_query,
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
        source_language: row.source_language,
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
    // IMP-REQ-003-04: explicit `?lang=` param > `lang` cookie >
    // `Accept-Language` header > `"en"` default, per `resolve_ui_locale`'s
    // verified precedence (IMP-REQ-003-03). Supersedes the plain
    // `detect_lang(&headers)` call REQ-001 originally used, which only
    // consulted `Accept-Language`.
    let accept_language = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let cookie_lang = extract_cookie_value(&headers, "lang");
    let lang = core::resolve_ui_locale(params.lang.as_deref(), cookie_lang, accept_language);
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

#[cfg(test)]
mod search_labels_tests {
    use super::*;

    // IMP-REQ-003-05: Rust's struct-literal exhaustiveness already
    // guarantees both the `"fr"` and `_` (default EN) arms of
    // `search_labels` construct the SAME `SearchLabels` type with ALL
    // fields populated — a field present in one language's arm but
    // missing from the other fails to compile. That rules out the
    // "missing key" class of bug that plagues loose key-value string
    // tables (HashMaps, JSON locale files, etc.), where nothing stops a
    // key from existing in one language file but not the other.
    //
    // What the compiler can't catch is a copy-paste mistake where a
    // translator (or a future PR) copies the EN value into the FR arm
    // (or vice versa) and forgets to actually translate it. This test
    // guards against that: it asserts every field differs between the
    // `en` and `fr` construction. If a new field is ever added to
    // `SearchLabels` and its EN/FR values are accidentally identical,
    // this test fails and forces a deliberate decision — either
    // translate it, or add it to the `identical_by_design` allowlist
    // below with a justifying comment (e.g. a proper noun that's the
    // same in both languages).
    #[test]
    fn en_and_fr_values_are_never_accidentally_identical() {
        let en = search_labels("en");
        let fr = search_labels("fr");

        // Fields legitimately identical across EN/FR belong here, by
        // name, with a reason. Currently empty: every existing
        // `SearchLabels` field has a genuinely distinct EN/FR wording.
        let identical_by_design: &[&str] = &[];

        let pairs: [(&str, &str, &str); 10] = [
            ("page_title", en.page_title, fr.page_title),
            ("heading", en.heading, fr.heading),
            ("search_label", en.search_label, fr.search_label),
            ("submit_label", en.submit_label, fr.submit_label),
            ("empty_message", en.empty_message, fr.empty_message),
            ("empty_guidance", en.empty_guidance, fr.empty_guidance),
            ("nav_permits", en.nav_permits, fr.nav_permits),
            ("nav_council", en.nav_council, fr.nav_council),
            (
                "municipality_select_label",
                en.municipality_select_label,
                fr.municipality_select_label,
            ),
            (
                "municipality_all_option",
                en.municipality_all_option,
                fr.municipality_all_option,
            ),
        ];

        for (field, en_value, fr_value) in pairs {
            if identical_by_design.contains(&field) {
                continue;
            }
            assert_ne!(
                en_value, fr_value,
                "SearchLabels.{field} is byte-identical between EN and FR \
                 ({en_value:?}) — looks like a copy-paste-forgot-to-translate \
                 mistake. If this is intentional (e.g. a proper noun), add \
                 \"{field}\" to `identical_by_design` with a comment \
                 explaining why.",
            );
        }
    }
}
