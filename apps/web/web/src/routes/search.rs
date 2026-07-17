use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::{
    routes::locale::{extract_cookie_value, resolve_ui_locale},
    AppState,
};

const DEFAULT_PER_PAGE: i64 = 20;
const MAX_PER_PAGE: i64 = 100;

/// Pure, I/O-free validation of raw search query params (IMP-REQ-001-02).
/// Extracted out of `run_search`'s inline `per_page` bounds check so the
/// validation rule is independently unit-testable without a DB/HTTP server.
/// Wired into `run_search` (IMP-REQ-001-04).
mod core {
    use super::{DEFAULT_PER_PAGE, MAX_PER_PAGE};
    use chrono::{DateTime, Duration, NaiveDate, Utc};

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
        /// `date_preset` was set to something other than the one supported
        /// value (`last_7_days`), or `date_from`/`date_to` failed to parse
        /// as `YYYY-MM-DD` (IMP-REQ-007-03). Maps to 400 (TC-007-4):
        /// distinct from `DateRangeInverted` below, which maps to 409, so
        /// callers can tell the two apart.
        MalformedDate,
        /// `date_from` parsed to a date strictly after `date_to` (TC-007-3).
        /// Kept distinct from `MalformedDate` (both dates are individually
        /// well-formed here — it's their combination that's invalid) so
        /// `run_search` can map it to 409 rather than 400.
        DateRangeInverted,
        /// `category`, after trimming, was neither the explicit
        /// `uncategorised` pseudo-value nor a syntactically plausible
        /// taxonomy code (lowercase ASCII alphanumerics/hyphens/underscores,
        /// within `MAX_CATEGORY_CODE_LEN`). Maps to 400 (TC-008-3):
        /// existence against the live `category_taxonomy` table is checked
        /// separately by the handler, same division of labor as
        /// `InvalidMunicipalitySlugFormat`/`validate_municipality_slug`.
        // Not yet raised by any caller — `validate_category` isn't wired
        // into `run_search` yet (that's IMP-REQ-008-04's job), so nothing
        // constructs this variant today. TC-008-3 (search_integration.rs)
        // exercises the eventual 400 mapping directly against the live
        // handler once that wiring lands.
        #[allow(dead_code)]
        InvalidCategoryFormat,
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

    /// Generous upper bound on a syntactically valid category code's
    /// length, mirroring `MAX_MUNICIPALITY_SLUG_LEN`'s role for
    /// `validate_municipality_slug`: real taxonomy codes (`residential`,
    /// `commercial`, `institutional`, `infrastructure`, `other`) are a
    /// handful of characters; this just guards against pathological input
    /// before it ever reaches a query.
    #[allow(dead_code)]
    const MAX_CATEGORY_CODE_LEN: usize = 100;

    /// The explicit pseudo-value matching `category_code IS NULL`
    /// (TC-008-2) — not a real row in `category_taxonomy`, so it must be
    /// recognized here, upstream of any table lookup the handler does.
    #[allow(dead_code)]
    pub const UNCATEGORISED: &str = "uncategorised";

    /// A validated `category` query param (IMP-REQ-008-03), ready for the
    /// handler to either check for existence against the live
    /// `category_taxonomy` table (`Code`) or apply directly as
    /// `category_code IS NULL` (`Uncategorised`).
    ///
    /// Loop A stub: not yet wired into `run_search` (IMP-REQ-008-04's job),
    /// so nothing outside this module's own unit tests constructs or
    /// matches on this type yet — `#[allow(dead_code)]` below follows the
    /// same precedent as `ProjectDetailContext`'s unwired fields in
    /// `routes/projects.rs`.
    #[allow(dead_code)]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CategoryFilter {
        /// A syntactically valid, not-yet-existence-checked taxonomy code.
        Code(String),
        /// The explicit `uncategorised` pseudo-value.
        Uncategorised,
    }

    /// Pure, syntactic-only validation of a raw `category` query param
    /// (IMP-REQ-008-03). Deliberately does NOT check whether the resulting
    /// code actually exists in the `category_taxonomy` table — that's a
    /// DB-touching concern layered on top by the handler, exactly mirroring
    /// `validate_municipality_slug`'s own split (syntax here, existence in
    /// the handler against the live table).
    ///
    /// - `None` or an empty/whitespace-only string means "no filter
    ///   applied", so it returns `Ok(None)`.
    /// - The value is trimmed and lowercased first, same normalization as
    ///   `validate_municipality_slug` (a case seen server-side, e.g.
    ///   `Residential`, signals a client normalization quirk, not an
    ///   implausible lookup).
    /// - After normalization, the literal `uncategorised` pseudo-value
    ///   (`UNCATEGORISED`) is recognized and returned as
    ///   `CategoryFilter::Uncategorised`, matching `category_code IS NULL`
    ///   (TC-008-2) — it is intentionally never checked against
    ///   `category_taxonomy` (it is not, and will never be, a real row
    ///   there).
    /// - Otherwise, anything other than lowercase ASCII alphanumerics,
    ///   hyphens, or underscores, or a length beyond
    ///   `MAX_CATEGORY_CODE_LEN`, is rejected as `InvalidCategoryFormat`
    ///   (TC-008-3 expects this to map to 400, before any project query
    ///   runs).
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    ///
    /// Not yet wired into `run_search` (IMP-REQ-008-04's job), which will
    /// layer the live-table existence check for `CategoryFilter::Code` on
    /// top, matching `validate_municipality_slug`'s own precedent.
    #[allow(dead_code)]
    pub fn validate_category(
        raw: Option<String>,
    ) -> Result<Option<CategoryFilter>, SearchValidationError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let normalized = trimmed.to_lowercase();
        if normalized == UNCATEGORISED {
            return Ok(Some(CategoryFilter::Uncategorised));
        }

        let is_valid_shape = normalized.len() <= MAX_CATEGORY_CODE_LEN
            && normalized
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !is_valid_shape {
            return Err(SearchValidationError::InvalidCategoryFormat);
        }

        Ok(Some(CategoryFilter::Code(normalized)))
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

    /// A validated `first_surfaced_at` date-range filter (IMP-REQ-007-03):
    /// `start`/`end` are both inclusive UTC bounds ready to bind directly
    /// into a `BETWEEN`-shaped SQL predicate.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DateRange {
        pub start: DateTime<Utc>,
        pub end: DateTime<Utc>,
    }

    /// The one supported `date_preset` value (IMP-REQ-007-03's confirmed
    /// param contract). Any other non-empty `date_preset` value is rejected
    /// as `MalformedDate` rather than silently ignored, since a caller that
    /// sent an unrecognized preset almost certainly made a mistake (a typo,
    /// or a client built against a preset this server doesn't support yet)
    /// rather than meaning "no filter".
    const PRESET_LAST_7_DAYS: &str = "last_7_days";

    /// Parses and validates the `date_preset`/`date_from`/`date_to` query
    /// params (IMP-REQ-007-03) into an optional `DateRange`, without
    /// touching the database. `now` is passed in explicitly (rather than
    /// read via `Utc::now()` inside this function) so `date_preset` is
    /// exercisable by pure, deterministic unit tests.
    ///
    /// - All three params absent (or `date_preset`/`date_from`/`date_to`
    ///   present but blank) means "no filter": returns `Ok(None)`.
    /// - `date_preset = "last_7_days"` computes `[now - 7 days, now]`,
    ///   ignoring any `date_from`/`date_to` also present (the preset takes
    ///   precedence — the two are alternative ways to specify a range, not
    ///   meant to be combined).
    /// - Any other non-blank `date_preset` value is rejected as
    ///   `MalformedDate`.
    /// - Otherwise, `date_from`/`date_to` (either or both may be present)
    ///   are each parsed as `YYYY-MM-DD` into UTC midnight boundaries:
    ///   `date_from` becomes that day's `00:00:00` UTC (inclusive, TC-007-6);
    ///   `date_to` becomes the LAST instant of that same day (`23:59:59.999999999`
    ///   UTC), so an inclusive `<=` comparison captures the whole day rather
    ///   than excluding everything after midnight. A missing `date_from`
    ///   defaults to the minimum representable `DateTime<Utc>`; a missing
    ///   `date_to` defaults to `now`, one-sided range.
    /// - A malformed (non-`YYYY-MM-DD`) `date_from`/`date_to` is rejected as
    ///   `MalformedDate` (TC-007-4).
    /// - `date_from > date_to` (both present and individually well-formed)
    ///   is rejected as `DateRangeInverted` (TC-007-3), distinct from
    ///   `MalformedDate` so callers can map it to a different status code
    ///   (409 vs 400).
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no environment
    /// reads (the only "clock" involved is the caller-supplied `now`).
    pub fn parse_date_filter(
        date_preset: Option<&str>,
        date_from: Option<&str>,
        date_to: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<DateRange>, SearchValidationError> {
        let date_preset = date_preset.filter(|s| !s.trim().is_empty());
        let date_from = date_from.filter(|s| !s.trim().is_empty());
        let date_to = date_to.filter(|s| !s.trim().is_empty());

        if let Some(preset) = date_preset {
            return if preset == PRESET_LAST_7_DAYS {
                Ok(Some(DateRange {
                    start: now - Duration::days(7),
                    end: now,
                }))
            } else {
                Err(SearchValidationError::MalformedDate)
            };
        }

        if date_from.is_none() && date_to.is_none() {
            return Ok(None);
        }

        let start = date_from
            .map(parse_ymd_start_of_day)
            .transpose()?
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        let end = date_to
            .map(parse_ymd_end_of_day)
            .transpose()?
            .unwrap_or(now);

        if start > end {
            return Err(SearchValidationError::DateRangeInverted);
        }

        Ok(Some(DateRange { start, end }))
    }

    /// Parses a `YYYY-MM-DD` string into that day's `00:00:00` UTC instant
    /// (inclusive lower bound).
    fn parse_ymd_start_of_day(raw: &str) -> Result<DateTime<Utc>, SearchValidationError> {
        let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|_| SearchValidationError::MalformedDate)?;
        Ok(date
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 is always a valid time")
            .and_utc())
    }

    /// Parses a `YYYY-MM-DD` string into that day's LAST representable UTC
    /// instant (`23:59:59.999999999`), so an inclusive `<=` comparison
    /// against it captures the entire day rather than excluding everything
    /// after midnight.
    fn parse_ymd_end_of_day(raw: &str) -> Result<DateTime<Utc>, SearchValidationError> {
        let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|_| SearchValidationError::MalformedDate)?;
        Ok(date
            .and_hms_nano_opt(23, 59, 59, 999_999_999)
            .expect("23:59:59.999999999 is always a valid time")
            .and_utc())
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

    /// Percent-encodes a single query-parameter value for safe inclusion in
    /// a URL (IMP-REQ-003-08). RFC 3986 unreserved characters (`A-Za-z0-9`,
    /// `-`, `_`, `.`, `~`) pass through unchanged; every other byte —
    /// including space and any multi-byte UTF-8 sequence, encoded one byte
    /// at a time — becomes a `%XX` escape. A minimal, purpose-built encoder
    /// scoped to what `build_lang_toggle_href` needs (there is no
    /// `url`/`percent-encoding` crate dependency in this workspace to reach
    /// for instead), not a general-purpose URL library.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn percent_encode_query_value(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char);
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    }

    /// Builds the `href` for the search page's EN/FR language-toggle link
    /// (IMP-REQ-003-08): always `/search` with `lang` set to the OTHER
    /// language than `current_lang` (the toggle's target), plus whichever of
    /// this page's two user-editable filter params — `q` and
    /// `municipality_slug` — are actually present, so following the link
    /// re-renders the same search in the other language instead of losing
    /// the user's current filters.
    ///
    /// Deliberately does NOT preserve the other, not-yet-wired `SearchParams`
    /// stub fields (`per_page`, `page`, `date_*`, `category`, `sort`) —
    /// `get_search_page` does not read or render any of them today, so
    /// there is nothing meaningful to preserve; adding them here would be
    /// speculative.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn build_lang_toggle_href(
        current_lang: &str,
        q: &str,
        municipality_slug: Option<&str>,
    ) -> String {
        let target_lang = if current_lang == "fr" { "en" } else { "fr" };

        let mut pairs: Vec<String> = Vec::new();
        if !q.is_empty() {
            pairs.push(format!("q={}", percent_encode_query_value(q)));
        }
        if let Some(slug) = municipality_slug.filter(|s| !s.is_empty()) {
            pairs.push(format!(
                "municipality_slug={}",
                percent_encode_query_value(slug)
            ));
        }
        pairs.push(format!("lang={target_lang}"));

        format!("/search?{}", pairs.join("&"))
    }

    /// Which of the three date-filter UI presets (IMP-REQ-007-09) is
    /// currently active, derived from the raw `date_preset`/`date_from`/
    /// `date_to` query params — used purely to decide which `<option>` in
    /// the search form's date-preset `<select>` renders `selected`
    /// (IMP-REQ-007-10), so the control reflects the current query-string
    /// state across a request the same way the municipality `<select>`
    /// already does.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DatePresetSelection {
        AnyTime,
        Last7Days,
        CustomRange,
    }

    impl DatePresetSelection {
        /// The value compared against in `search.html`'s `{% if %}` guards.
        pub fn as_str(&self) -> &'static str {
            match self {
                DatePresetSelection::AnyTime => "any_time",
                DatePresetSelection::Last7Days => "last_7_days",
                DatePresetSelection::CustomRange => "custom_range",
            }
        }
    }

    /// Determines which preset option should render as `selected`
    /// (IMP-REQ-007-10). Mirrors `parse_date_filter`'s own precedence
    /// exactly (a non-blank `date_preset` wins over `date_from`/`date_to`),
    /// so the UI's selected preset always agrees with what the backend will
    /// actually filter on for these same raw params.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn selected_date_preset(
        date_preset: Option<&str>,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> DatePresetSelection {
        let date_preset = date_preset.filter(|s| !s.trim().is_empty());
        let date_from = date_from.filter(|s| !s.trim().is_empty());
        let date_to = date_to.filter(|s| !s.trim().is_empty());

        if date_preset == Some(PRESET_LAST_7_DAYS) {
            return DatePresetSelection::Last7Days;
        }
        if date_from.is_some() || date_to.is_some() {
            return DatePresetSelection::CustomRange;
        }
        DatePresetSelection::AnyTime
    }

    /// Builds the applied-filter chip's label text (IMP-REQ-007-10), shown
    /// alongside a removable "x" whenever a date filter is currently active.
    /// Returns `None` when no date filter is active (no chip renders) or
    /// when `date_preset` is some unrecognized, non-`last_7_days` value (the
    /// backend would already have rejected such a request with 400 before
    /// this ever gets called with a real request's params, so this is just
    /// a safe default rather than a case expected to occur in practice).
    ///
    /// Pure string formatting from already-known raw param values — no I/O.
    pub fn format_date_filter_chip_label(
        lang: &str,
        date_preset: Option<&str>,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> Option<String> {
        let date_preset = date_preset.filter(|s| !s.trim().is_empty());
        let date_from = date_from.filter(|s| !s.trim().is_empty());
        let date_to = date_to.filter(|s| !s.trim().is_empty());

        if let Some(preset) = date_preset {
            return if preset == PRESET_LAST_7_DAYS {
                Some(if lang == "fr" {
                    "7 derniers jours".to_string()
                } else {
                    "Last 7 days".to_string()
                })
            } else {
                None
            };
        }

        match (date_from, date_to) {
            (Some(from), Some(to)) => Some(if lang == "fr" {
                format!("Du {from} au {to}")
            } else {
                format!("From {from} to {to}")
            }),
            (Some(from), None) => Some(if lang == "fr" {
                format!("Depuis le {from}")
            } else {
                format!("From {from}")
            }),
            (None, Some(to)) => Some(if lang == "fr" {
                format!("Jusqu'au {to}")
            } else {
                format!("Until {to}")
            }),
            (None, None) => None,
        }
    }

    /// Builds the `href` for the applied date-filter chip's "x" clear link
    /// (IMP-REQ-007-10): always `/search` with the current `lang` plus
    /// whichever of `q`/`municipality_slug` are actually present — the same
    /// param-preservation pattern as `build_lang_toggle_href`/
    /// `build_pagination_href` — but with NO `date_preset`/`date_from`/
    /// `date_to` params at all, so following it clears just the date filter
    /// while leaving every other active filter untouched.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn build_clear_date_filter_href(
        lang: &str,
        q: &str,
        municipality_slug: Option<&str>,
    ) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if !q.is_empty() {
            pairs.push(format!("q={}", percent_encode_query_value(q)));
        }
        if let Some(slug) = municipality_slug.filter(|s| !s.is_empty()) {
            pairs.push(format!(
                "municipality_slug={}",
                percent_encode_query_value(slug)
            ));
        }
        pairs.push(format!("lang={lang}"));

        format!("/search?{}", pairs.join("&"))
    }

    /// Builds the `href` for the search-results pagination "Next"/"Previous"
    /// links (IMP-REQ-004-06), following the same param-preservation pattern
    /// as `build_lang_toggle_href` (IMP-REQ-003-08): always `/search` with
    /// `page` set to `target_page`, plus whichever of `q`/`municipality_slug`
    /// are actually present, plus the current `lang` (so paging forward/back
    /// doesn't lose the page's rendering language). Unlike
    /// `build_lang_toggle_href`, `lang` is always included (not just the
    /// non-default one) since there is no "other" language to compute here —
    /// this link must simply preserve whatever language the page is
    /// currently rendering in.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn build_pagination_href(
        lang: &str,
        q: &str,
        municipality_slug: Option<&str>,
        target_page: i64,
    ) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if !q.is_empty() {
            pairs.push(format!("q={}", percent_encode_query_value(q)));
        }
        if let Some(slug) = municipality_slug.filter(|s| !s.is_empty()) {
            pairs.push(format!(
                "municipality_slug={}",
                percent_encode_query_value(slug)
            ));
        }
        pairs.push(format!("page={target_page}"));
        pairs.push(format!("lang={lang}"));

        format!("/search?{}", pairs.join("&"))
    }

    /// Pagination math shared by the JSON envelope (IMP-REQ-004-04) and the
    /// HTML fragment's next/prev branching (IMP-REQ-004-05). Pure
    /// data-in/data-out over an already-known `total` row count — no
    /// database access, no HTTP, no clock, no environment reads.
    ///
    /// `page` is 1-indexed. Any `page < 1` is clamped to `1` rather than
    /// rejected: an out-of-range page is defined by this task's plan as "not
    /// an error" (TC-004-5 pins this for a too-high page; a too-low/negative
    /// page is treated the same way for consistency, simply clamped to the
    /// first page instead of the last).
    ///
    /// `per_page` is expected to have already passed
    /// `validate_search_params` (so it is `>= 1`); a non-positive `per_page`
    /// is defensively clamped to `1` here too so `offset`/`has_more` stay
    /// well-defined even if a caller skips validation, rather than this
    /// function dividing-by-zero or returning a nonsensical negative
    /// `offset`.
    ///
    /// `offset = (page - 1) * per_page`. `has_more` is `true` when at least
    /// one further row exists past this page's window
    /// (`offset + per_page < total`); a `page` far beyond the last page
    /// yields a large `offset`, which callers use as a `LIMIT`/`OFFSET` SQL
    /// clause that naturally returns zero rows, and `has_more` correctly
    /// comes back `false` since `offset` alone already exceeds `total`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PaginationInfo {
        pub page: i64,
        pub per_page: i64,
        pub total: i64,
        pub offset: i64,
        pub has_more: bool,
    }

    pub fn paginate(total: i64, page: i64, per_page: i64) -> PaginationInfo {
        let page = page.max(1);
        let per_page = per_page.max(1);
        let offset = (page - 1) * per_page;
        let has_more = offset + per_page < total;
        PaginationInfo {
            page,
            per_page,
            total,
            offset,
            has_more,
        }
    }

    /// Synthesizes a `SearchResult`'s display name from its civic address
    /// and (optional) project type (IMP-REQ-004-09), e.g. `project_type =
    /// Some("demolition")`, `civic_address = "123 main st"` yields
    /// `"Demolition — 123 main st"`. See `SearchResult::display_name`'s doc
    /// comment for the target-state shape this feeds.
    ///
    /// `project_type` is free text from the extraction pipeline (no fixed
    /// enum backs it — see `projects.project_type TEXT`), so this only
    /// capitalizes its first character for a presentable label; it does not
    /// otherwise reformat or translate the value. `civic_address` is passed
    /// through unchanged — it is not this function's job to re-normalize an
    /// address that `civic_address_normalized` has already normalized
    /// upstream.
    ///
    /// A `None` or blank/whitespace-only `project_type` is treated as "no
    /// category to prefix" and the civic address is returned alone, without
    /// a dangling separator.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn synthesize_display_name(civic_address: &str, project_type: Option<&str>) -> String {
        match project_type.map(str::trim).filter(|s| !s.is_empty()) {
            Some(project_type) => format!(
                "{} — {}",
                capitalize_first_char(project_type),
                civic_address
            ),
            None => civic_address.to_string(),
        }
    }

    /// Capitalizes only the first character of `s`, leaving the rest
    /// untouched (so e.g. an already-mixed-case or accented value isn't
    /// mangled beyond its leading character). Empty input returns empty
    /// output.
    fn capitalize_first_char(s: &str) -> String {
        let mut chars = s.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
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

        /// A fixed reference instant used across `parse_date_filter` tests so
        /// they are deterministic rather than depending on the real clock.
        fn fixed_now() -> DateTime<Utc> {
            DateTime::parse_from_rfc3339("2026-07-15T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        }

        #[test]
        fn date_filter_no_params_means_no_filter() {
            let result = parse_date_filter(None, None, None, fixed_now()).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn date_filter_blank_params_mean_no_filter() {
            let result = parse_date_filter(Some(""), Some("  "), None, fixed_now()).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn date_filter_last_7_days_preset_computes_now_minus_7_days() {
            let now = fixed_now();
            let result = parse_date_filter(Some("last_7_days"), None, None, now)
                .unwrap()
                .unwrap();
            assert_eq!(result.start, now - Duration::days(7));
            assert_eq!(result.end, now);
        }

        #[test]
        fn date_filter_unrecognized_preset_is_rejected_as_malformed() {
            let result = parse_date_filter(Some("last_month"), None, None, fixed_now());
            assert_eq!(result, Err(SearchValidationError::MalformedDate));
        }

        #[test]
        fn date_filter_preset_takes_precedence_over_date_from_to() {
            // The preset and an explicit custom range are alternative ways to
            // specify a filter, not meant to be combined; the preset wins.
            let now = fixed_now();
            let result = parse_date_filter(
                Some("last_7_days"),
                Some("2020-01-01"),
                Some("2020-01-02"),
                now,
            )
            .unwrap()
            .unwrap();
            assert_eq!(result.start, now - Duration::days(7));
            assert_eq!(result.end, now);
        }

        #[test]
        fn date_filter_valid_custom_range_parses_both_bounds() {
            let result = parse_date_filter(
                None,
                Some("2026-06-01"),
                Some("2026-06-30"),
                fixed_now(),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                result.start,
                DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            );
            assert_eq!(
                result.end,
                NaiveDate::from_ymd_opt(2026, 6, 30)
                    .unwrap()
                    .and_hms_nano_opt(23, 59, 59, 999_999_999)
                    .unwrap()
                    .and_utc()
            );
        }

        #[test]
        fn date_filter_only_date_from_defaults_end_to_now() {
            let now = fixed_now();
            let result = parse_date_filter(None, Some("2026-06-01"), None, now)
                .unwrap()
                .unwrap();
            assert_eq!(
                result.start,
                DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            );
            assert_eq!(result.end, now);
        }

        #[test]
        fn date_filter_only_date_to_defaults_start_to_minimum() {
            let result = parse_date_filter(None, None, Some("2026-06-30"), fixed_now())
                .unwrap()
                .unwrap();
            assert_eq!(result.start, DateTime::<Utc>::MIN_UTC);
        }

        #[test]
        fn date_filter_malformed_date_from_is_rejected() {
            let result = parse_date_filter(None, Some("not-a-date"), None, fixed_now());
            assert_eq!(result, Err(SearchValidationError::MalformedDate));
        }

        #[test]
        fn date_filter_malformed_date_to_is_rejected() {
            let result = parse_date_filter(None, None, Some("2026-13-40"), fixed_now());
            assert_eq!(result, Err(SearchValidationError::MalformedDate));
        }

        #[test]
        fn date_filter_wrong_format_is_rejected() {
            // ISO 8601 with a time component, not the expected YYYY-MM-DD.
            let result = parse_date_filter(None, Some("2026-06-01T00:00:00Z"), None, fixed_now());
            assert_eq!(result, Err(SearchValidationError::MalformedDate));
        }

        #[test]
        fn date_filter_date_from_after_date_to_is_rejected() {
            let result = parse_date_filter(
                None,
                Some("2026-07-20"),
                Some("2026-07-10"),
                fixed_now(),
            );
            assert_eq!(result, Err(SearchValidationError::DateRangeInverted));
        }

        #[test]
        fn date_filter_date_from_equals_date_to_is_accepted() {
            let result = parse_date_filter(
                None,
                Some("2026-07-10"),
                Some("2026-07-10"),
                fixed_now(),
            )
            .unwrap()
            .unwrap();
            assert!(result.start <= result.end);
        }

        /// Boundary: a project surfaced at exactly midnight UTC on
        /// `date_from` is inside the range (inclusive); one exactly one
        /// second before is outside it (TC-007-6).
        #[test]
        fn date_filter_date_from_boundary_is_inclusive_at_midnight() {
            let result = parse_date_filter(None, Some("2026-07-10"), None, fixed_now())
                .unwrap()
                .unwrap();
            let midnight = NaiveDate::from_ymd_opt(2026, 7, 10)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            let one_second_before = midnight - Duration::seconds(1);

            assert!(midnight >= result.start, "midnight boundary must be inside the range");
            assert!(
                one_second_before < result.start,
                "one second before midnight must be outside the range"
            );
        }

        /// Boundary: a project surfaced at the last instant of `date_to`'s
        /// day is inside the range (inclusive); the first instant of the
        /// following day is outside it.
        #[test]
        fn date_filter_date_to_boundary_is_inclusive_through_end_of_day() {
            let result = parse_date_filter(None, None, Some("2026-07-10"), fixed_now())
                .unwrap()
                .unwrap();
            let last_instant_of_day = NaiveDate::from_ymd_opt(2026, 7, 10)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, 999_999_999)
                .unwrap()
                .and_utc();
            let start_of_next_day = NaiveDate::from_ymd_opt(2026, 7, 11)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();

            assert!(
                last_instant_of_day <= result.end,
                "last instant of date_to's day must be inside the range"
            );
            assert!(
                start_of_next_day > result.end,
                "the following day must be outside the range"
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
        fn category_none_input_means_no_filter() {
            let result = validate_category(None).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn category_empty_string_means_no_filter() {
            let result = validate_category(Some(String::new())).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn category_whitespace_only_is_trimmed_to_no_filter() {
            let result = validate_category(Some("   ".to_string())).unwrap();
            assert_eq!(result, None);
        }

        #[test]
        fn category_valid_lowercase_code_is_accepted_unchanged() {
            let result = validate_category(Some("residential".to_string())).unwrap();
            assert_eq!(result, Some(CategoryFilter::Code("residential".to_string())));
        }

        #[test]
        fn category_uppercase_is_normalized_to_lowercase() {
            let result = validate_category(Some("RESIDENTIAL".to_string())).unwrap();
            assert_eq!(result, Some(CategoryFilter::Code("residential".to_string())));
        }

        #[test]
        fn category_surrounding_whitespace_is_trimmed() {
            let result = validate_category(Some("  residential  ".to_string())).unwrap();
            assert_eq!(result, Some(CategoryFilter::Code("residential".to_string())));
        }

        #[test]
        fn category_uncategorised_pseudo_value_is_recognized() {
            let result = validate_category(Some("uncategorised".to_string())).unwrap();
            assert_eq!(result, Some(CategoryFilter::Uncategorised));
        }

        #[test]
        fn category_uncategorised_pseudo_value_is_case_and_whitespace_insensitive() {
            let result = validate_category(Some("  Uncategorised  ".to_string())).unwrap();
            assert_eq!(result, Some(CategoryFilter::Uncategorised));
        }

        #[test]
        fn category_with_hyphen_and_underscore_is_accepted() {
            let result = validate_category(Some("mixed-use_v2".to_string())).unwrap();
            assert_eq!(
                result,
                Some(CategoryFilter::Code("mixed-use_v2".to_string()))
            );
        }

        #[test]
        fn category_with_invalid_characters_is_rejected() {
            let result = validate_category(Some("not a real category!".to_string()));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidCategoryFormat)
            );
        }

        #[test]
        fn category_with_accented_characters_is_rejected() {
            let result = validate_category(Some("résidentiel".to_string()));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidCategoryFormat)
            );
        }

        #[test]
        fn category_too_long_is_rejected() {
            let too_long = "a".repeat(MAX_CATEGORY_CODE_LEN + 1);
            let result = validate_category(Some(too_long));
            assert_eq!(
                result,
                Err(SearchValidationError::InvalidCategoryFormat)
            );
        }

        #[test]
        fn category_at_max_length_is_accepted() {
            let max_len = "a".repeat(MAX_CATEGORY_CODE_LEN);
            let result = validate_category(Some(max_len.clone())).unwrap();
            assert_eq!(result, Some(CategoryFilter::Code(max_len)));
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
        fn percent_encode_query_value_leaves_unreserved_characters_unchanged() {
            assert_eq!(
                percent_encode_query_value("saint-denis_2024.v1~a"),
                "saint-denis_2024.v1~a"
            );
        }

        #[test]
        fn percent_encode_query_value_encodes_spaces() {
            assert_eq!(percent_encode_query_value("rue saint-denis"), "rue%20saint-denis");
        }

        #[test]
        fn percent_encode_query_value_encodes_multi_byte_utf8_one_byte_at_a_time() {
            // "é" is the two-byte UTF-8 sequence 0xC3 0xA9.
            assert_eq!(percent_encode_query_value("montréal"), "montr%C3%A9al");
        }

        #[test]
        fn percent_encode_query_value_encodes_ampersand_and_equals() {
            assert_eq!(percent_encode_query_value("a&b=c"), "a%26b%3Dc");
        }

        #[test]
        fn build_lang_toggle_href_targets_french_from_english_page_with_no_filters() {
            assert_eq!(build_lang_toggle_href("en", "", None), "/search?lang=fr");
        }

        #[test]
        fn build_lang_toggle_href_targets_english_from_french_page_with_no_filters() {
            assert_eq!(build_lang_toggle_href("fr", "", None), "/search?lang=en");
        }

        /// Any current lang other than exactly `"fr"` is treated as English,
        /// matching the same fallback convention as
        /// `format_result_count_label`/`format_municipality_empty_message`.
        #[test]
        fn build_lang_toggle_href_treats_unrecognized_current_lang_as_english() {
            assert_eq!(build_lang_toggle_href("xx", "", None), "/search?lang=fr");
        }

        #[test]
        fn build_lang_toggle_href_preserves_present_query_and_municipality_slug() {
            assert_eq!(
                build_lang_toggle_href("en", "saint-denis", Some("montreal")),
                "/search?q=saint-denis&municipality_slug=montreal&lang=fr"
            );
        }

        #[test]
        fn build_lang_toggle_href_omits_empty_query_and_absent_municipality_slug() {
            assert_eq!(build_lang_toggle_href("en", "", None), "/search?lang=fr");
            assert_eq!(
                build_lang_toggle_href("en", "", Some("")),
                "/search?lang=fr"
            );
        }

        #[test]
        fn build_lang_toggle_href_percent_encodes_the_preserved_query_value() {
            assert_eq!(
                build_lang_toggle_href("en", "rue saint-denis", None),
                "/search?q=rue%20saint-denis&lang=fr"
            );
        }

        #[test]
        fn build_pagination_href_next_page_with_no_filters() {
            assert_eq!(
                build_pagination_href("en", "", None, 2),
                "/search?page=2&lang=en"
            );
        }

        #[test]
        fn build_pagination_href_preserves_query_and_municipality_slug() {
            assert_eq!(
                build_pagination_href("fr", "saint-denis", Some("montreal"), 3),
                "/search?q=saint-denis&municipality_slug=montreal&page=3&lang=fr"
            );
        }

        #[test]
        fn build_pagination_href_omits_empty_query_and_absent_municipality_slug() {
            assert_eq!(
                build_pagination_href("en", "", Some(""), 1),
                "/search?page=1&lang=en"
            );
        }

        #[test]
        fn build_pagination_href_percent_encodes_the_preserved_query_value() {
            assert_eq!(
                build_pagination_href("en", "rue saint-denis", None, 2),
                "/search?q=rue%20saint-denis&page=2&lang=en"
            );
        }

        #[test]
        fn selected_date_preset_no_params_is_any_time() {
            assert_eq!(
                selected_date_preset(None, None, None),
                DatePresetSelection::AnyTime
            );
        }

        #[test]
        fn selected_date_preset_blank_params_is_any_time() {
            assert_eq!(
                selected_date_preset(Some(""), Some("  "), None),
                DatePresetSelection::AnyTime
            );
        }

        #[test]
        fn selected_date_preset_last_7_days_wins() {
            assert_eq!(
                selected_date_preset(Some("last_7_days"), None, None),
                DatePresetSelection::Last7Days
            );
        }

        /// Mirrors `parse_date_filter`'s own precedence: a `last_7_days`
        /// preset wins over any `date_from`/`date_to` also present.
        #[test]
        fn selected_date_preset_last_7_days_wins_over_custom_range_params() {
            assert_eq!(
                selected_date_preset(Some("last_7_days"), Some("2026-01-01"), Some("2026-02-01")),
                DatePresetSelection::Last7Days
            );
        }

        #[test]
        fn selected_date_preset_date_from_only_is_custom_range() {
            assert_eq!(
                selected_date_preset(None, Some("2026-01-01"), None),
                DatePresetSelection::CustomRange
            );
        }

        #[test]
        fn selected_date_preset_date_to_only_is_custom_range() {
            assert_eq!(
                selected_date_preset(None, None, Some("2026-02-01")),
                DatePresetSelection::CustomRange
            );
        }

        #[test]
        fn selected_date_preset_both_dates_is_custom_range() {
            assert_eq!(
                selected_date_preset(None, Some("2026-01-01"), Some("2026-02-01")),
                DatePresetSelection::CustomRange
            );
        }

        #[test]
        fn date_preset_selection_as_str_matches_template_comparison_values() {
            assert_eq!(DatePresetSelection::AnyTime.as_str(), "any_time");
            assert_eq!(DatePresetSelection::Last7Days.as_str(), "last_7_days");
            assert_eq!(DatePresetSelection::CustomRange.as_str(), "custom_range");
        }

        #[test]
        fn format_date_filter_chip_label_no_filter_is_none() {
            assert_eq!(format_date_filter_chip_label("en", None, None, None), None);
        }

        #[test]
        fn format_date_filter_chip_label_last_7_days_english() {
            assert_eq!(
                format_date_filter_chip_label("en", Some("last_7_days"), None, None),
                Some("Last 7 days".to_string())
            );
        }

        #[test]
        fn format_date_filter_chip_label_last_7_days_french() {
            assert_eq!(
                format_date_filter_chip_label("fr", Some("last_7_days"), None, None),
                Some("7 derniers jours".to_string())
            );
        }

        #[test]
        fn format_date_filter_chip_label_both_dates_english() {
            assert_eq!(
                format_date_filter_chip_label("en", None, Some("2026-01-01"), Some("2026-02-01")),
                Some("From 2026-01-01 to 2026-02-01".to_string())
            );
        }

        #[test]
        fn format_date_filter_chip_label_both_dates_french() {
            assert_eq!(
                format_date_filter_chip_label("fr", None, Some("2026-01-01"), Some("2026-02-01")),
                Some("Du 2026-01-01 au 2026-02-01".to_string())
            );
        }

        #[test]
        fn format_date_filter_chip_label_from_only() {
            assert_eq!(
                format_date_filter_chip_label("en", None, Some("2026-01-01"), None),
                Some("From 2026-01-01".to_string())
            );
        }

        #[test]
        fn format_date_filter_chip_label_to_only() {
            assert_eq!(
                format_date_filter_chip_label("en", None, None, Some("2026-02-01")),
                Some("Until 2026-02-01".to_string())
            );
        }

        #[test]
        fn format_date_filter_chip_label_unrecognized_preset_is_none() {
            assert_eq!(
                format_date_filter_chip_label("en", Some("last_month"), None, None),
                None
            );
        }

        #[test]
        fn build_clear_date_filter_href_with_no_other_filters() {
            assert_eq!(
                build_clear_date_filter_href("en", "", None),
                "/search?lang=en"
            );
        }

        #[test]
        fn build_clear_date_filter_href_preserves_query_and_municipality_slug() {
            assert_eq!(
                build_clear_date_filter_href("fr", "saint-denis", Some("montreal")),
                "/search?q=saint-denis&municipality_slug=montreal&lang=fr"
            );
        }

        #[test]
        fn build_clear_date_filter_href_percent_encodes_the_preserved_query_value() {
            assert_eq!(
                build_clear_date_filter_href("en", "rue saint-denis", None),
                "/search?q=rue%20saint-denis&lang=en"
            );
        }

        #[test]
        fn paginate_first_page_offset_is_zero() {
            let info = paginate(50, 1, 20);
            assert_eq!(info.offset, 0);
            assert_eq!(info.page, 1);
            assert_eq!(info.per_page, 20);
            assert_eq!(info.total, 50);
            assert!(info.has_more);
        }

        #[test]
        fn paginate_middle_page_offset_and_has_more() {
            let info = paginate(50, 2, 20);
            assert_eq!(info.offset, 20);
            assert!(info.has_more);
        }

        #[test]
        fn paginate_last_full_page_has_no_more() {
            // total=50, per_page=20: page 3 covers rows 40..50 exactly.
            let info = paginate(50, 3, 20);
            assert_eq!(info.offset, 40);
            assert!(!info.has_more);
        }

        #[test]
        fn paginate_exact_multiple_boundary_has_no_more() {
            // total is an exact multiple of per_page: the last page's window
            // ends precisely at `total`, so there is nothing further.
            let info = paginate(40, 2, 20);
            assert_eq!(info.offset, 20);
            assert!(!info.has_more);
        }

        #[test]
        fn paginate_page_beyond_the_last_page_yields_empty_window_and_no_more() {
            // TC-004-5: an out-of-range page must not be an error; the
            // resulting offset simply lands past `total`, which a
            // LIMIT/OFFSET query turns into zero rows, and has_more is false.
            let info = paginate(1, 999, 20);
            assert_eq!(info.offset, 999 * 20 - 20);
            assert!(!info.has_more);
        }

        #[test]
        fn paginate_zero_total_never_has_more() {
            let info = paginate(0, 1, 20);
            assert_eq!(info.offset, 0);
            assert!(!info.has_more);
        }

        #[test]
        fn paginate_page_zero_is_clamped_to_first_page() {
            let info = paginate(50, 0, 20);
            assert_eq!(info.page, 1);
            assert_eq!(info.offset, 0);
        }

        #[test]
        fn paginate_negative_page_is_clamped_to_first_page() {
            let info = paginate(50, -5, 20);
            assert_eq!(info.page, 1);
            assert_eq!(info.offset, 0);
        }

        #[test]
        fn paginate_non_positive_per_page_is_defensively_clamped_to_one() {
            let info = paginate(50, 1, 0);
            assert_eq!(info.per_page, 1);
            assert_eq!(info.offset, 0);

            let info = paginate(50, 3, -10);
            assert_eq!(info.per_page, 1);
            assert_eq!(info.offset, 2);
        }

        #[test]
        fn paginate_single_result_single_page_has_no_more() {
            let info = paginate(1, 1, 20);
            assert_eq!(info.offset, 0);
            assert!(!info.has_more);
        }

        #[test]
        fn synthesize_display_name_with_project_type_capitalizes_and_joins_with_em_dash() {
            assert_eq!(
                synthesize_display_name("123 main st", Some("demolition")),
                "Demolition — 123 main st"
            );
        }

        #[test]
        fn synthesize_display_name_preserves_already_capitalized_project_type() {
            assert_eq!(
                synthesize_display_name("123 main st", Some("Demolition")),
                "Demolition — 123 main st"
            );
        }

        #[test]
        fn synthesize_display_name_none_project_type_returns_address_alone() {
            assert_eq!(
                synthesize_display_name("123 main st", None),
                "123 main st"
            );
        }

        #[test]
        fn synthesize_display_name_blank_project_type_returns_address_alone() {
            assert_eq!(synthesize_display_name("123 main st", Some("")), "123 main st");
            assert_eq!(
                synthesize_display_name("123 main st", Some("   ")),
                "123 main st"
            );
        }

        #[test]
        fn synthesize_display_name_trims_surrounding_whitespace_on_project_type() {
            assert_eq!(
                synthesize_display_name("123 main st", Some("  demolition  ")),
                "Demolition — 123 main st"
            );
        }

        #[test]
        fn synthesize_display_name_handles_multi_byte_utf8_first_character() {
            // "é" is a multi-byte UTF-8 scalar; capitalization must not panic
            // or corrupt it, and only the leading character is affected.
            assert_eq!(
                synthesize_display_name("123 rue principale", Some("étude")),
                "Étude — 123 rue principale"
            );
        }

        #[test]
        fn synthesize_display_name_empty_civic_address_still_prefixes_project_type() {
            assert_eq!(
                synthesize_display_name("", Some("demolition")),
                "Demolition — "
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
    // IMP-REQ-004-04: 1-indexed page number, wired into `run_search`'s
    // `LIMIT`/`OFFSET` window via `core::paginate`. Absent defaults to page 1.
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
    // IMP-REQ-004-09: synthesized display name (civic address + project
    // type, e.g. "Demolition — 123 Main St") via
    // `core::synthesize_display_name`, populated by `run_search`.
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

/// Paginated envelope returned by `GET /api/v1/projects/search`
/// (IMP-REQ-004-04, TC-004-1): `results` is this page's slice, `total` the
/// full unpaginated match count, and `has_more` whether a further page
/// exists — all derived from `core::paginate`.
#[derive(Debug, Serialize)]
pub struct SearchResultsEnvelope {
    pub results: Vec<SearchResult>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub has_more: bool,
}

/// Raw (unvalidated) `date_preset`/`date_from`/`date_to` query params,
/// bundled into a single struct so `run_search` takes one argument for the
/// three rather than three more positional parameters (see its doc comment).
#[derive(Debug, Clone, Copy)]
struct RawDateFilterQuery<'a> {
    date_preset: Option<&'a str>,
    date_from: Option<&'a str>,
    date_to: Option<&'a str>,
}

/// Validates `per_page` (TC-REQ-008-3: rejected before any DB query runs)
/// and runs the keyword search shared by both the JSON API and the
/// server-rendered page. `q` matches against either the civic address or
/// the municipality name (TC-REQ-008-2: a query that only matches the
/// municipality, with no address-keyword overlap, still returns results).
///
/// IMP-REQ-004-04: also computes `total` — a `COUNT(*)` over the SAME
/// `WHERE` clause as the main `SELECT` (kept textually side-by-side with it
/// below so the two can't silently drift apart) — and applies `page`'s
/// `LIMIT`/`OFFSET` window (via `core::paginate`) to the main query, so the
/// returned `Vec<SearchResult>` is only this page's slice while the returned
/// `core::PaginationInfo` reflects the full unpaginated match count and
/// whether a further page remains.
///
/// `date_filter` bundles the raw, unvalidated `date_preset`/`date_from`/
/// `date_to` query params into a single argument (rather than three more
/// positional parameters) purely to stay under clippy's
/// `too_many_arguments` threshold.
async fn run_search(
    pool: &sqlx::PgPool,
    q: &str,
    per_page: Option<i64>,
    municipality_slug: Option<String>,
    page: Option<i64>,
    date_filter: RawDateFilterQuery<'_>,
    category: Option<String>,
) -> Result<(Vec<SearchResult>, core::PaginationInfo), StatusCode> {
    let core::ValidatedSearchParams { per_page } =
        core::validate_search_params(per_page).map_err(|_| StatusCode::BAD_REQUEST)?;

    // IMP-REQ-002-04: syntactic validation first (TC-002-2/-6), before any
    // DB query runs.
    let municipality_slug = core::validate_municipality_slug(municipality_slug)
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // IMP-REQ-008-05: syntactic validation of `category` (TC-008-3), same
    // split as `municipality_slug` above — `core::validate_category` only
    // checks shape/recognizes the `uncategorised` pseudo-value; existence of
    // a real code against the live `category_taxonomy` table is checked
    // below, once it's known the value isn't the pseudo-value.
    let category_filter =
        core::validate_category(category).map_err(|_| StatusCode::BAD_REQUEST)?;

    // IMP-REQ-007-05: `date_preset`/`date_from`/`date_to` parsing/validation
    // also runs before any DB query, matching the `municipality_slug`
    // pattern immediately above. A malformed date (TC-007-4) maps to 400; an
    // inverted `date_from > date_to` range (TC-007-3) maps to 409, distinct
    // from every other validation failure in this function so far.
    let date_range = core::parse_date_filter(
        date_filter.date_preset,
        date_filter.date_from,
        date_filter.date_to,
        chrono::Utc::now(),
    )
    .map_err(|err| match err {
        core::SearchValidationError::DateRangeInverted => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    })?;
    let date_start = date_range.map(|range| range.start);
    let date_end = date_range.map(|range| range.end);

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

    // IMP-REQ-008-05: mirrors the `municipality_slug` existence check above
    // — a syntactically valid `category` code must also exist in the live
    // `category_taxonomy` table (TC-008-3), except for the `uncategorised`
    // pseudo-value, which is never a real row there by design (see
    // `core::validate_category`'s doc comment) and so skips this check
    // entirely. `category_code_eq`/`require_uncategorised` are the two
    // mutually-exclusive query params bound below: at most one ever narrows
    // the result set, matching `core::CategoryFilter`'s own exclusivity.
    let mut category_code_eq: Option<String> = None;
    let mut require_uncategorised = false;
    match category_filter {
        None => {}
        Some(core::CategoryFilter::Uncategorised) => {
            require_uncategorised = true;
        }
        Some(core::CategoryFilter::Code(code)) => {
            let exists = sqlx::query_scalar!(
                "SELECT EXISTS(SELECT 1 FROM category_taxonomy WHERE code = $1)",
                code
            )
            .fetch_one(pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .unwrap_or(false);

            if !exists {
                return Err(StatusCode::BAD_REQUEST);
            }
            category_code_eq = Some(code);
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

    // IMP-REQ-004-04: total match count over the exact same filter
    // predicate as the paginated SELECT below, so `total` can never drift
    // from what `page`/`per_page` are actually windowing over.
    let total = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*)
        FROM public_search_documents
        WHERE (
            civic_address_normalized ILIKE $1
            OR municipality_name ILIKE $1
            OR search_vector_fr @@ plainto_tsquery('french', $2)
            OR search_vector_en @@ plainto_tsquery('english', $2)
        )
          AND ($3::text IS NULL OR municipality_slug = $3)
          AND ($4::timestamptz IS NULL OR first_surfaced_at >= $4)
          AND ($5::timestamptz IS NULL OR first_surfaced_at <= $5)
          AND ($6::text IS NULL OR category_code = $6)
          AND (NOT $7::bool OR category_code IS NULL)
        "#,
        keyword,
        normalized_query,
        municipality_slug,
        date_start,
        date_end,
        category_code_eq,
        require_uncategorised,
    )
    .fetch_one(pool)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .unwrap_or(0);

    let pagination = core::paginate(total, page.unwrap_or(1), per_page);

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
          AND ($6::timestamptz IS NULL OR first_surfaced_at >= $6)
          AND ($7::timestamptz IS NULL OR first_surfaced_at <= $7)
          AND ($8::text IS NULL OR category_code = $8)
          AND (NOT $9::bool OR category_code IS NULL)
        ORDER BY civic_address_normalized ASC
        LIMIT $2 OFFSET $5
        "#,
        keyword,
        pagination.per_page,
        municipality_slug,
        normalized_query,
        pagination.offset,
        date_start,
        date_end,
        category_code_eq,
        require_uncategorised,
    )
    .fetch_all(pool)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .into_iter()
    .map(|row| SearchResult {
        project_id: row.project_id,
        civic_address_normalized: row.civic_address_normalized.clone(),
        municipality_name: row.municipality_name,
        // IMP-REQ-004-09: synthesized from the civic address + project type
        // (e.g. "Demolition — 123 Main St") rather than left as the prior
        // `None` stub.
        display_name: Some(core::synthesize_display_name(
            &row.civic_address_normalized,
            row.project_type.as_deref(),
        )),
        project_type: row.project_type,
        normalized_status: row.normalized_status,
        source_language: row.source_language,
        // Loop A stub: see `SearchResult::first_detected_at`/`source_count`
        // doc comment — IMP-REQ-015-02/03/04 migration+materializer.
        first_detected_at: None,
        source_count: None,
    })
    .collect();

    Ok((rows, pagination))
}

/// GET /api/v1/projects/search — public, unauthenticated keyword search
/// (TC-REQ-008-1..4). Returns the paginated `SearchResultsEnvelope`
/// (IMP-REQ-004-04, TC-004-1), not a bare array.
pub async fn search_projects(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResultsEnvelope>, StatusCode> {
    let (results, pagination) = run_search(
        &state.db,
        &params.q,
        params.per_page,
        params.municipality_slug,
        params.page,
        RawDateFilterQuery {
            date_preset: params.date_preset.as_deref(),
            date_from: params.date_from.as_deref(),
            date_to: params.date_to.as_deref(),
        },
        params.category,
    )
    .await?;

    Ok(Json(SearchResultsEnvelope {
        results,
        total: pagination.total,
        page: pagination.page,
        per_page: pagination.per_page,
        has_more: pagination.has_more,
    }))
}

/// GET /categories — public, unauthenticated category facet endpoint
/// (IMP-REQ-008-04, TC-008-4/-5). Returns the public rows of
/// `category_taxonomy` (`is_public = true`) as a plain list of codes, in
/// `sort_order` (migration 023), matching TC-008-4's exact expected
/// `["residential", "commercial", "institutional", "infrastructure",
/// "other"]` list — not `{code, label_en, label_fr}` objects; the plan left
/// the exact shape to whatever TC-008-4 asserts, and TC-008-4 asserts a bare
/// array of codes.
///
/// Degrades to 503 (TC-008-5) if the underlying query fails (e.g. DB
/// unavailable), mirroring `run_search`'s existing pool-unavailable mapping,
/// rather than crashing the whole search page this facet feeds.
pub async fn list_categories(State(state): State<AppState>) -> Result<Json<Vec<String>>, StatusCode> {
    let codes = sqlx::query_scalar!(
        "SELECT code FROM category_taxonomy WHERE is_public ORDER BY sort_order"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Json(codes))
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
    // IMP-REQ-003-08: the EN/FR toggle link's own text — the TARGET
    // language's own name (e.g. "Français" shown on the English page), not a
    // translation of "switch language". Keyed by the CURRENT page's `lang`
    // like every other `SearchLabels` field, so `search_labels("en")` names
    // French and `search_labels("fr")` names English.
    lang_toggle_label: &'static str,
    // IMP-REQ-004-06: text for the results pagination "Next"/"Previous"
    // links.
    pagination_next_label: &'static str,
    pagination_previous_label: &'static str,
    // IMP-REQ-007-08/-09: labels for the date-filter UI. `date_filter_label`
    // is the `<label for>` text of the preset `<select>`;
    // `date_preset_*_option` are that select's three option labels;
    // `date_from_label`/`date_to_label` are the custom-range date inputs'
    // own `<label for>` text; `date_filter_clear_label` is the applied
    // filter chip's "x" accessible clear-text (IMP-REQ-007-11: never a bare
    // "x" glyph alone).
    date_filter_label: &'static str,
    date_preset_any_time_option: &'static str,
    date_preset_last_7_days_option: &'static str,
    date_preset_custom_range_option: &'static str,
    date_from_label: &'static str,
    date_to_label: &'static str,
    date_filter_clear_label: &'static str,
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
            lang_toggle_label: "English",
            pagination_next_label: "Suivant",
            pagination_previous_label: "Précédent",
            date_filter_label: "Filtre de date",
            date_preset_any_time_option: "Toute période",
            date_preset_last_7_days_option: "7 derniers jours",
            date_preset_custom_range_option: "Plage personnalisée",
            date_from_label: "Depuis",
            date_to_label: "Jusqu'au",
            date_filter_clear_label: "Effacer le filtre de date",
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
            lang_toggle_label: "Français",
            pagination_next_label: "Next",
            pagination_previous_label: "Previous",
            date_filter_label: "Date filter",
            date_preset_any_time_option: "Any time",
            date_preset_last_7_days_option: "Last 7 days",
            date_preset_custom_range_option: "Custom range",
            date_from_label: "From",
            date_to_label: "To",
            date_filter_clear_label: "Clear date filter",
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
) -> Result<(HeaderMap, Html<String>), StatusCode> {
    // IMP-REQ-003-04: explicit `?lang=` param > `lang` cookie >
    // `Accept-Language` header > `"en"` default, per `resolve_ui_locale`'s
    // verified precedence (IMP-REQ-003-03). Supersedes the plain
    // `detect_lang(&headers)` call REQ-001 originally used, which only
    // consulted `Accept-Language`.
    let accept_language = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let cookie_lang = extract_cookie_value(&headers, "lang");
    let lang = resolve_ui_locale(params.lang.as_deref(), cookie_lang, accept_language);
    let labels = search_labels(lang);

    // IMP-REQ-004-05: htmx marks EVERY request it issues with
    // `HX-Request: true` (see https://htmx.org/docs/#request-headers). When
    // present, the request originated from an in-page htmx interaction (e.g.
    // the eventual infinite-scroll "load more" trigger wired by
    // IMP-REQ-004-07) that only needs to swap the results region, so this
    // handler renders `results_fragment.html` alone rather than the full
    // `search.html` page with its `<html>`/`<head>`/nav chrome — swapping a
    // full document into a fragment-sized target would duplicate that chrome
    // into the page on every subsequent request. A plain browser navigation
    // (no `HX-Request` header) always gets the full page, exactly as before.
    let is_htmx_request = headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let template_name = if is_htmx_request {
        "results_fragment.html"
    } else {
        "search.html"
    };
    let tmpl = state
        .env
        .get_template(template_name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let has_searched = !params.q.is_empty();
    let search_outcome = if has_searched {
        Some(
            run_search(
                &state.db,
                &params.q,
                params.per_page,
                params.municipality_slug.clone(),
                params.page,
                RawDateFilterQuery {
                    date_preset: params.date_preset.as_deref(),
                    date_from: params.date_from.as_deref(),
                    date_to: params.date_to.as_deref(),
                },
                params.category.clone(),
            )
            .await,
        )
    } else {
        None
    };

    // IMP-REQ-004-06: `run_search`'s `PaginationInfo` is applied above (via
    // `params.page` flowing into the `run_search` call) to window the SQL
    // `LIMIT`/`OFFSET`, so `search_results` is already just this page's
    // slice. `pagination` itself is threaded into the template context below
    // so the results fragment can render real "Next"/"Previous" pagination
    // controls (`has_more`/`page`) rather than discarding it.
    let (search_results, search_error, pagination) = match search_outcome {
        Some(Ok((results, pagination))) => (results, false, Some(pagination)),
        Some(Err(_)) => (Vec::new(), true, None),
        None => (Vec::new(), false, None),
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

    // IMP-REQ-003-08: the toggle always targets the OTHER language than
    // this page's resolved `lang`, preserving the `q`/`municipality_slug`
    // filters actually in play so following the link re-renders the same
    // search rather than losing the user's current filters.
    let lang_toggle_href =
        core::build_lang_toggle_href(lang, &params.q, params.municipality_slug.as_deref());

    // IMP-REQ-007-10: which of the three date-filter presets currently
    // renders `selected` in the search form, derived from the same raw
    // `date_preset`/`date_from`/`date_to` params `run_search` above already
    // validated (or rejected) — so the control reflects the CURRENT query
    // string state across a request, matching the municipality `<select>`'s
    // existing selection-preservation behavior.
    let date_preset_selection = core::selected_date_preset(
        params.date_preset.as_deref(),
        params.date_from.as_deref(),
        params.date_to.as_deref(),
    );

    // IMP-REQ-007-10: the applied-filter chip's label, shown only when a
    // date filter is actually active; `None` renders no chip at all.
    let date_filter_chip_label = core::format_date_filter_chip_label(
        lang,
        params.date_preset.as_deref(),
        params.date_from.as_deref(),
        params.date_to.as_deref(),
    );

    // IMP-REQ-007-10: the chip's "x" clear-link href — preserves `q`/
    // `municipality_slug`/`lang` but omits every date param, so following it
    // removes just the date filter.
    let clear_date_filter_href =
        core::build_clear_date_filter_href(lang, &params.q, params.municipality_slug.as_deref());

    // IMP-REQ-004-06: pagination controls only ever accompany a non-empty
    // rendered result list — an empty/error/pre-search state has no page to
    // move forward/back from. `current_page`/`has_more` drive the
    // fragment's next/prev branching; `next_page_href`/`prev_page_href` are
    // pre-built via `core::build_pagination_href` (same param-preservation
    // pattern as the lang toggle) so the template does no URL construction
    // of its own.
    let (current_page, has_more, next_page_href, prev_page_href) = match &pagination {
        Some(p) if !search_results.is_empty() => (
            Some(p.page),
            p.has_more,
            p.has_more.then(|| {
                core::build_pagination_href(
                    lang,
                    &params.q,
                    params.municipality_slug.as_deref(),
                    p.page + 1,
                )
            }),
            (p.page > 1).then(|| {
                core::build_pagination_href(
                    lang,
                    &params.q,
                    params.municipality_slug.as_deref(),
                    p.page - 1,
                )
            }),
        ),
        _ => (None, false, None, None),
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
            lang_toggle_href => lang_toggle_href,
            lang_toggle_label => labels.lang_toggle_label,
            current_page => current_page,
            has_more => has_more,
            next_page_href => next_page_href,
            prev_page_href => prev_page_href,
            pagination_next_label => labels.pagination_next_label,
            pagination_previous_label => labels.pagination_previous_label,
            date_filter_label => labels.date_filter_label,
            date_preset_any_time_option => labels.date_preset_any_time_option,
            date_preset_last_7_days_option => labels.date_preset_last_7_days_option,
            date_preset_custom_range_option => labels.date_preset_custom_range_option,
            date_from_label => labels.date_from_label,
            date_to_label => labels.date_to_label,
            date_filter_clear_label => labels.date_filter_clear_label,
            date_preset_selection => date_preset_selection.as_str(),
            date_from_value => params.date_from,
            date_to_value => params.date_to,
            date_filter_chip_label => date_filter_chip_label,
            clear_date_filter_href => clear_date_filter_href,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // IMP-REQ-003-08: persists the resolved locale as a `lang` cookie so a
    // later request with no explicit `?lang=` param (a plain revisit, or
    // navigating to a different page) still renders in this same language
    // via the cookie, per `resolve_ui_locale`'s cookie precedence tier.
    // `HttpOnly` is chosen deliberately: the toggle is a plain `<a href>`
    // link, not JS-driven, so nothing client-side ever needs to read this
    // cookie's value — keeping it `HttpOnly` costs nothing here and is
    // slightly safer (not readable by any injected/third-party script).
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_str(&format!("lang={lang}; Path=/; SameSite=Lax; HttpOnly"))
            .expect("lang cookie value is always the ASCII literal \"en\" or \"fr\""),
    );

    Ok((response_headers, Html(html)))
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

        let pairs: [(&str, &str, &str); 20] = [
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
            (
                "lang_toggle_label",
                en.lang_toggle_label,
                fr.lang_toggle_label,
            ),
            (
                "pagination_next_label",
                en.pagination_next_label,
                fr.pagination_next_label,
            ),
            (
                "pagination_previous_label",
                en.pagination_previous_label,
                fr.pagination_previous_label,
            ),
            (
                "date_filter_label",
                en.date_filter_label,
                fr.date_filter_label,
            ),
            (
                "date_preset_any_time_option",
                en.date_preset_any_time_option,
                fr.date_preset_any_time_option,
            ),
            (
                "date_preset_last_7_days_option",
                en.date_preset_last_7_days_option,
                fr.date_preset_last_7_days_option,
            ),
            (
                "date_preset_custom_range_option",
                en.date_preset_custom_range_option,
                fr.date_preset_custom_range_option,
            ),
            ("date_from_label", en.date_from_label, fr.date_from_label),
            ("date_to_label", en.date_to_label, fr.date_to_label),
            (
                "date_filter_clear_label",
                en.date_filter_clear_label,
                fr.date_filter_clear_label,
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
