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
        /// `sort`, after trimming and lowercasing, was neither `relevance`
        /// nor `date` (TC-009-5). Maps to 400, raised before any project
        /// query runs, matching `InvalidCategoryFormat`/`validate_category`'s
        /// own division of labor.
        InvalidSortValue,
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

    /// A validated `sort` query param (IMP-REQ-009-04): which ordering
    /// `run_search` should apply. `Relevance` is the existing default
    /// ordering (`civic_address_normalized` ASC, TC-009-2 regression guard);
    /// `Date` orders by `latest_meeting_date DESC NULLS LAST` (TC-009-1/-3),
    /// tie-broken by `civic_address_normalized ASC` (TC-009-4).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SortOrder {
        Relevance,
        Date,
    }

    /// Pure, syntactic validation of a raw `sort` query param
    /// (IMP-REQ-009-04), mirroring `validate_category`'s own shape:
    /// normalize first, then recognize a small fixed set of literal values,
    /// rejecting anything else BEFORE any query runs (TC-009-5).
    ///
    /// - `None` or an empty/whitespace-only string means "no explicit sort
    ///   requested", defaulting to `SortOrder::Relevance` (TC-009-2: omitted
    ///   `sort` preserves today's default ordering).
    /// - The value is trimmed and lowercased first, same normalization as
    ///   `validate_category`/`validate_municipality_slug`.
    /// - `"relevance"` maps to `SortOrder::Relevance`, `"date"` to
    ///   `SortOrder::Date`.
    /// - Anything else is rejected as `InvalidSortValue` (TC-009-5), mapped
    ///   to 400 by the caller.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn validate_sort(raw: Option<String>) -> Result<SortOrder, SearchValidationError> {
        let Some(raw) = raw else {
            return Ok(SortOrder::Relevance);
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(SortOrder::Relevance);
        }

        match trimmed.to_lowercase().as_str() {
            "relevance" => Ok(SortOrder::Relevance),
            "date" => Ok(SortOrder::Date),
            _ => Err(SearchValidationError::InvalidSortValue),
        }
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

    /// Bundles the raw filter params that every "preserve the user's other
    /// active filters, but change just this one thing" href builder for the
    /// category chip row (IMP-REQ-008-07) needs to carry forward: `q`/
    /// `municipality_slug` mirror `build_lang_toggle_href`'s own preserved
    /// fields, and `date_preset`/`date_from`/`date_to` are the raw
    /// (unvalidated) date-filter params, so switching category doesn't
    /// silently drop whatever date filter is currently active. Bundled into
    /// one struct (rather than five more positional parameters) purely to
    /// stay clear of clippy's `too_many_arguments` threshold, same reasoning
    /// as `RawDateFilterQuery` for `run_search`.
    #[derive(Debug, Clone, Copy)]
    pub struct FilterHrefContext<'a> {
        pub lang: &'a str,
        pub q: &'a str,
        pub municipality_slug: Option<&'a str>,
        pub date_preset: Option<&'a str>,
        pub date_from: Option<&'a str>,
        pub date_to: Option<&'a str>,
        // IMP-REQ-009-08: the raw (unvalidated) `sort` query param, so
        // clicking a category chip (`build_category_filter_href`) doesn't
        // silently drop back to the default `relevance` ordering when the
        // user had `sort=date` active — same "preserve every other filter"
        // principle as `date_preset`/`date_from`/`date_to` above.
        pub sort: Option<&'a str>,
        // IMP-REQ-009-08: the raw (already-normalized-or-not) `category`
        // query param, so toggling `sort` (`build_sort_toggle_href`) doesn't
        // silently drop an active category filter — same reasoning as
        // `sort` above, just the other direction.
        pub category: Option<&'a str>,
    }

    /// Builds the `href` for a category filter chip (IMP-REQ-008-07): always
    /// `/search` with `category` set to `category` (omitted entirely for the
    /// "All categories" chip, i.e. `category = None`), plus whichever of
    /// `q`/`municipality_slug`/`date_preset`/`date_from`/`date_to` in `ctx`
    /// are actually present, plus the current `lang` — so clicking a chip
    /// re-runs the same search scoped to (or cleared of) a category without
    /// losing any other filter already in play, matching
    /// `build_lang_toggle_href`/`build_pagination_href`'s own
    /// param-preservation precedent.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn build_category_filter_href(ctx: FilterHrefContext<'_>, category: Option<&str>) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if !ctx.q.is_empty() {
            pairs.push(format!("q={}", percent_encode_query_value(ctx.q)));
        }
        if let Some(slug) = ctx.municipality_slug.filter(|s| !s.is_empty()) {
            pairs.push(format!(
                "municipality_slug={}",
                percent_encode_query_value(slug)
            ));
        }
        if let Some(preset) = ctx.date_preset.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!(
                "date_preset={}",
                percent_encode_query_value(preset)
            ));
        }
        if let Some(from) = ctx.date_from.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!("date_from={}", percent_encode_query_value(from)));
        }
        if let Some(to) = ctx.date_to.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!("date_to={}", percent_encode_query_value(to)));
        }
        if let Some(code) = category.filter(|c| !c.is_empty()) {
            pairs.push(format!("category={}", percent_encode_query_value(code)));
        }
        if let Some(sort) = ctx.sort.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!("sort={}", percent_encode_query_value(sort)));
        }
        pairs.push(format!("lang={}", ctx.lang));

        format!("/search?{}", pairs.join("&"))
    }

    /// Builds the `href` for the sort-toggle button/link pair
    /// (IMP-REQ-009-08): always `/search` with `sort` set to
    /// `target_sort`'s literal query value (omitted entirely for
    /// `SortOrder::Relevance`, mirroring `build_category_filter_href`'s own
    /// "omit the param for the default state" convention for its "All
    /// categories" chip), plus whichever of `q`/`municipality_slug`/
    /// `date_preset`/`date_from`/`date_to`/`category` in `ctx` are actually
    /// present, plus the current `lang` — so toggling sort doesn't lose any
    /// other filter already in play, matching every other href builder in
    /// this module's own param-preservation precedent.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn build_sort_toggle_href(ctx: FilterHrefContext<'_>, target_sort: SortOrder) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if !ctx.q.is_empty() {
            pairs.push(format!("q={}", percent_encode_query_value(ctx.q)));
        }
        if let Some(slug) = ctx.municipality_slug.filter(|s| !s.is_empty()) {
            pairs.push(format!(
                "municipality_slug={}",
                percent_encode_query_value(slug)
            ));
        }
        if let Some(preset) = ctx.date_preset.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!(
                "date_preset={}",
                percent_encode_query_value(preset)
            ));
        }
        if let Some(from) = ctx.date_from.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!("date_from={}", percent_encode_query_value(from)));
        }
        if let Some(to) = ctx.date_to.filter(|s| !s.trim().is_empty()) {
            pairs.push(format!("date_to={}", percent_encode_query_value(to)));
        }
        if let Some(code) = ctx
            .category
            .filter(|c| !c.trim().is_empty())
        {
            pairs.push(format!("category={}", percent_encode_query_value(code)));
        }
        if target_sort == SortOrder::Date {
            pairs.push("sort=date".to_string());
        }
        pairs.push(format!("lang={}", ctx.lang));

        format!("/search?{}", pairs.join("&"))
    }

    /// Maps a category taxonomy code to its localized display label
    /// (IMP-REQ-008-10), matching `municipality_display_name`'s pattern:
    /// covers the launch taxonomy (`residential`, `commercial`,
    /// `institutional`, `infrastructure`, `other` — the same five codes
    /// `GET /categories` returns), returning `None` for any code outside
    /// that set rather than guessing at a label. Expects `code` to already be
    /// lowercase-normalized (as the live `category_taxonomy` table's `code`
    /// column and `validate_category` both are); this function does not
    /// itself lowercase or trim.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn category_display_name(code: &str, lang: &str) -> Option<&'static str> {
        match (code, lang) {
            ("residential", "fr") => Some("Résidentiel"),
            ("residential", _) => Some("Residential"),
            ("commercial", "fr") => Some("Commercial"),
            ("commercial", _) => Some("Commercial"),
            ("institutional", "fr") => Some("Institutionnel"),
            ("institutional", _) => Some("Institutional"),
            ("infrastructure", "fr") => Some("Infrastructures"),
            ("infrastructure", _) => Some("Infrastructure"),
            ("other", "fr") => Some("Autre"),
            ("other", _) => Some("Other"),
            _ => None,
        }
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

    /// IMP-REQ-015-03: pure formatting of the "confidence indicator"
    /// sentence ("Detected N day(s) ago from M council source(s)") shown on
    /// a search result card (IMP-REQ-015-06/-11) and mirrored on the
    /// project-detail page (`projects::core::format_detection_sentence`).
    ///
    /// Returns `None` — omitting the sentence entirely, not a partial
    /// fragment (TC-015-5) — whenever `first_detected_at` is `None`,
    /// `source_count` is `None`, or `source_count` is non-positive (treated
    /// the same as "unknown": there is no meaningful "detected from 0
    /// sources" sentence to show).
    ///
    /// Day count is a *calendar-day* difference (`.date_naive()` on both
    /// sides, matching this codebase's existing UTC-midnight-boundary
    /// convention — see `tc_007_6_date_from_boundary_is_inclusive`), not a
    /// 24-hour-bucket difference: `0` renders the explicit word "today"
    /// (TC-015-3), `1` the singular "1 day ago" (never "1 days ago"), and
    /// any larger value the plural "N days ago". `source_count` is
    /// singular/plural the same way: "1 council source" vs. "N council
    /// sources" (TC-015-4). A `first_detected_at` in the future (clock skew
    /// or a data anomaly) clamps to `0` ("today") rather than producing a
    /// nonsensical negative day count.
    ///
    /// Pure data-in/data-out: `now` is passed in by the caller (read once
    /// via `Utc::now()` in the shell) rather than read internally, so this
    /// function needs no clock access of its own and is fully
    /// unit-testable (IMP-REQ-015-08).
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

    /// IMP-REQ-011-08: pure decision of whether a request's fault-injection
    /// signals — a `force_fault` query param or `X-Force-Fault` header,
    /// both checked against the literal value `"503"` — request an injected
    /// fault. Either signal alone is sufficient (TC-011-5 sends both).
    /// Whether this hook is honored AT ALL is a separate, environment-gated
    /// decision the caller (`get_search_page`) makes — only in debug
    /// builds, never in a release build — so this function only decides
    /// what the RAW signals mean, independent of that gating, and is
    /// unit-testable without any environment/cfg concerns.
    ///
    /// Pure data-in/data-out: no database access, no HTTP, no clock, no
    /// environment reads.
    pub fn should_force_fault(query_param: Option<&str>, header: Option<&str>) -> bool {
        query_param == Some("503") || header == Some("503")
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
        fn sort_none_input_defaults_to_relevance() {
            assert_eq!(validate_sort(None).unwrap(), SortOrder::Relevance);
        }

        #[test]
        fn sort_empty_string_defaults_to_relevance() {
            assert_eq!(validate_sort(Some(String::new())).unwrap(), SortOrder::Relevance);
        }

        #[test]
        fn sort_whitespace_only_defaults_to_relevance() {
            assert_eq!(
                validate_sort(Some("   ".to_string())).unwrap(),
                SortOrder::Relevance
            );
        }

        #[test]
        fn sort_relevance_literal_is_accepted() {
            assert_eq!(
                validate_sort(Some("relevance".to_string())).unwrap(),
                SortOrder::Relevance
            );
        }

        #[test]
        fn sort_date_literal_is_accepted() {
            assert_eq!(
                validate_sort(Some("date".to_string())).unwrap(),
                SortOrder::Date
            );
        }

        #[test]
        fn sort_uppercase_is_normalized_to_lowercase() {
            assert_eq!(
                validate_sort(Some("DATE".to_string())).unwrap(),
                SortOrder::Date
            );
        }

        #[test]
        fn sort_surrounding_whitespace_is_trimmed() {
            assert_eq!(
                validate_sort(Some("  date  ".to_string())).unwrap(),
                SortOrder::Date
            );
        }

        #[test]
        fn sort_invalid_value_is_rejected() {
            let result = validate_sort(Some("alphabetical".to_string()));
            assert_eq!(result, Err(SearchValidationError::InvalidSortValue));
        }

        #[test]
        fn sort_valid_looking_but_unrecognized_value_is_rejected() {
            // "relevancy" is not "relevance" — must not fuzzy-match.
            let result = validate_sort(Some("relevancy".to_string()));
            assert_eq!(result, Err(SearchValidationError::InvalidSortValue));
        }

        #[test]
        fn build_sort_toggle_href_relevance_omits_sort_param() {
            let ctx = no_filters_ctx("en");
            assert_eq!(
                build_sort_toggle_href(ctx, SortOrder::Relevance),
                "/search?lang=en"
            );
        }

        #[test]
        fn build_sort_toggle_href_date_includes_sort_param() {
            let ctx = no_filters_ctx("en");
            assert_eq!(
                build_sort_toggle_href(ctx, SortOrder::Date),
                "/search?sort=date&lang=en"
            );
        }

        #[test]
        fn build_sort_toggle_href_preserves_query_and_municipality_slug() {
            let ctx = FilterHrefContext {
                lang: "fr",
                q: "saint-denis",
                municipality_slug: Some("montreal"),
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            };
            assert_eq!(
                build_sort_toggle_href(ctx, SortOrder::Date),
                "/search?q=saint-denis&municipality_slug=montreal&sort=date&lang=fr"
            );
        }

        #[test]
        fn build_sort_toggle_href_preserves_active_category() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "",
                municipality_slug: None,
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: Some("residential"),
            };
            assert_eq!(
                build_sort_toggle_href(ctx, SortOrder::Date),
                "/search?category=residential&sort=date&lang=en"
            );
        }

        #[test]
        fn build_sort_toggle_href_percent_encodes_the_preserved_query_value() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "rue saint-denis",
                municipality_slug: None,
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            };
            assert_eq!(
                build_sort_toggle_href(ctx, SortOrder::Relevance),
                "/search?q=rue%20saint-denis&lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_preserves_active_sort() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "",
                municipality_slug: None,
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: Some("date"),
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, Some("residential")),
                "/search?category=residential&sort=date&lang=en"
            );
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

        /// Builds a `FilterHrefContext` with every field defaulted to
        /// "absent" so individual tests below only need to override what
        /// they're actually exercising.
        fn no_filters_ctx(lang: &str) -> FilterHrefContext<'_> {
            FilterHrefContext {
                lang,
                q: "",
                municipality_slug: None,
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            }
        }

        #[test]
        fn build_category_filter_href_no_filters_all_categories() {
            // category = None is the "All categories" chip: the `category`
            // param must be omitted entirely, not sent as an empty string.
            assert_eq!(
                build_category_filter_href(no_filters_ctx("en"), None),
                "/search?lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_no_filters_specific_category() {
            assert_eq!(
                build_category_filter_href(no_filters_ctx("en"), Some("residential")),
                "/search?category=residential&lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_preserves_query_and_municipality_slug() {
            let ctx = FilterHrefContext {
                lang: "fr",
                q: "saint-denis",
                municipality_slug: Some("montreal"),
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, Some("commercial")),
                "/search?q=saint-denis&municipality_slug=montreal&category=commercial&lang=fr"
            );
        }

        #[test]
        fn build_category_filter_href_preserves_date_preset() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "",
                municipality_slug: None,
                date_preset: Some("last_7_days"),
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, Some("other")),
                "/search?date_preset=last_7_days&category=other&lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_preserves_date_from_and_date_to() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "",
                municipality_slug: None,
                date_preset: None,
                date_from: Some("2026-01-01"),
                date_to: Some("2026-01-31"),
                sort: None,
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, None),
                "/search?date_from=2026-01-01&date_to=2026-01-31&lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_omits_empty_query_and_absent_municipality_slug() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "",
                municipality_slug: Some(""),
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, Some("residential")),
                "/search?category=residential&lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_omits_blank_whitespace_only_date_params() {
            // Mirrors `parse_date_filter`/`format_date_filter_chip_label`'s
            // own treatment of blank date params as "absent" — a
            // present-but-whitespace-only `date_preset`/`date_from`/
            // `date_to` must not leak into the href as an empty/whitespace
            // query param.
            let ctx = FilterHrefContext {
                lang: "en",
                q: "",
                municipality_slug: None,
                date_preset: Some(""),
                date_from: Some("   "),
                date_to: Some(""),
                sort: None,
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, None),
                "/search?lang=en"
            );
        }

        #[test]
        fn build_category_filter_href_percent_encodes_the_preserved_query_value() {
            let ctx = FilterHrefContext {
                lang: "en",
                q: "rue saint-denis",
                municipality_slug: None,
                date_preset: None,
                date_from: None,
                date_to: None,
                sort: None,
                category: None,
            };
            assert_eq!(
                build_category_filter_href(ctx, None),
                "/search?q=rue%20saint-denis&lang=en"
            );
        }

        #[test]
        fn category_display_name_residential_english() {
            assert_eq!(
                category_display_name("residential", "en"),
                Some("Residential")
            );
        }

        #[test]
        fn category_display_name_residential_french() {
            assert_eq!(
                category_display_name("residential", "fr"),
                Some("Résidentiel")
            );
        }

        #[test]
        fn category_display_name_commercial_english() {
            assert_eq!(
                category_display_name("commercial", "en"),
                Some("Commercial")
            );
        }

        #[test]
        fn category_display_name_commercial_french() {
            assert_eq!(
                category_display_name("commercial", "fr"),
                Some("Commercial")
            );
        }

        #[test]
        fn category_display_name_institutional_english() {
            assert_eq!(
                category_display_name("institutional", "en"),
                Some("Institutional")
            );
        }

        #[test]
        fn category_display_name_institutional_french() {
            assert_eq!(
                category_display_name("institutional", "fr"),
                Some("Institutionnel")
            );
        }

        #[test]
        fn category_display_name_infrastructure_english() {
            assert_eq!(
                category_display_name("infrastructure", "en"),
                Some("Infrastructure")
            );
        }

        #[test]
        fn category_display_name_infrastructure_french() {
            assert_eq!(
                category_display_name("infrastructure", "fr"),
                Some("Infrastructures")
            );
        }

        #[test]
        fn category_display_name_other_english() {
            assert_eq!(category_display_name("other", "en"), Some("Other"));
        }

        #[test]
        fn category_display_name_other_french() {
            assert_eq!(category_display_name("other", "fr"), Some("Autre"));
        }

        #[test]
        fn category_display_name_unrecognized_code_returns_none() {
            assert_eq!(category_display_name("gotham", "en"), None);
            assert_eq!(category_display_name("gotham", "fr"), None);
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

        // -- format_detection_sentence (IMP-REQ-015-03/-08) --

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
            assert_eq!(
                format_detection_sentence("en", Some(now), Some(-1), now),
                None
            );
        }

        #[test]
        fn format_detection_sentence_zero_days_renders_today() {
            let now = Utc::now();
            assert_eq!(
                format_detection_sentence("en", Some(now), Some(1), now),
                Some("Detected today from 1 council source".to_string())
            );
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
        fn format_detection_sentence_future_first_detected_at_clamps_to_today() {
            let now = Utc::now();
            let in_the_future = now + chrono::Duration::days(3);
            let sentence = format_detection_sentence("en", Some(in_the_future), Some(1), now);
            assert_eq!(
                sentence,
                Some("Detected today from 1 council source".to_string())
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

        // -- should_force_fault (IMP-REQ-011-08) --

        #[test]
        fn should_force_fault_neither_signal_present_is_false() {
            assert!(!should_force_fault(None, None));
        }

        #[test]
        fn should_force_fault_query_param_alone_is_sufficient() {
            assert!(should_force_fault(Some("503"), None));
        }

        #[test]
        fn should_force_fault_header_alone_is_sufficient() {
            assert!(should_force_fault(None, Some("503")));
        }

        #[test]
        fn should_force_fault_both_signals_present_is_true() {
            assert!(should_force_fault(Some("503"), Some("503")));
        }

        #[test]
        fn should_force_fault_wrong_value_is_ignored() {
            assert!(!should_force_fault(Some("500"), None));
            assert!(!should_force_fault(None, Some("200")));
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
    // IMP-REQ-009-04: validated via `core::validate_sort` (TC-009-5: an
    // unrecognized value is rejected with 400 before any query runs), then
    // wired into `run_search`'s `ORDER BY` (IMP-REQ-009-06). `None` or
    // `"relevance"` preserves today's default ordering
    // (`civic_address_normalized` ASC); `"date"` orders by
    // `latest_meeting_date DESC NULLS LAST`.
    pub sort: Option<String>,
    // IMP-REQ-003-04: explicit UI-locale override, highest-precedence input
    // to `core::resolve_ui_locale`.
    pub lang: Option<String>,
    // IMP-REQ-011-08: test-only fault-injection signal (TC-011-5), read
    // alongside the `X-Force-Fault` header by `core::should_force_fault`.
    // Only honored in debug builds (see `get_search_page`) — production
    // (release) builds never read this field, so it can't be used to force
    // a real outage against a live deployment.
    pub force_fault: Option<String>,
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

/// A single category filter chip in the search form's chip row
/// (IMP-REQ-008-07): `code` is the raw taxonomy code the chip's `href`
/// filters on, `label` is its localized display text (via
/// `core::category_display_name`, falling back to the raw code for any
/// taxonomy entry outside today's launch set, mirroring
/// `MunicipalityOption::display_name`'s own DB-`name` fallback),
/// `selected` drives the chip's `aria-current`/visual-selected styling
/// (IMP-REQ-008-14: never color-only), and `href` is pre-built via
/// `core::build_category_filter_href` so the template does no URL
/// construction of its own.
#[derive(Debug, Serialize)]
pub struct CategoryChip {
    pub code: String,
    pub label: String,
    pub href: String,
    pub selected: bool,
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
    // IMP-REQ-015-02/04: populated from `public_search_documents`, kept in
    // sync by the refresh job's materializer alongside `latest_meeting_date`.
    pub first_detected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub source_count: Option<i64>,
    // IMP-REQ-015-06/-11: "Detected N day(s) ago from M council source(s)",
    // derived from `first_detected_at`/`source_count` via
    // `core::format_detection_sentence` once `lang`/`now` are known (in the
    // route shell, not here) — `None` whenever either underlying field is
    // `None` (TC-015-5), never a partial fragment.
    pub detection_sentence: Option<String>,
    // IMP-REQ-009-06: mirrors `public_search_documents.latest_meeting_date`
    // (migration 024) — the most recent council meeting date this project
    // has been discussed at, `None` for a project with no timeline events
    // yet. Populated regardless of the active `sort` (not just when
    // `sort=date`), so the per-row meeting-date display (IMP-REQ-009-09) has
    // it available whenever it exists.
    pub latest_meeting_date: Option<chrono::DateTime<chrono::Utc>>,
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

/// Raw (unvalidated) `category`/`sort` query params, bundled into a single
/// struct for the same `too_many_arguments`-avoidance reason as
/// `RawDateFilterQuery` above (IMP-REQ-009-04).
#[derive(Debug, Clone)]
struct RawResultFilterQuery {
    category: Option<String>,
    sort: Option<String>,
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
    result_filter: RawResultFilterQuery,
) -> Result<(Vec<SearchResult>, core::PaginationInfo), StatusCode> {
    let RawResultFilterQuery { category, sort } = result_filter;
    let core::ValidatedSearchParams { per_page } =
        core::validate_search_params(per_page).map_err(|_| StatusCode::BAD_REQUEST)?;

    // IMP-REQ-009-04: syntactic validation of `sort` (TC-009-5), before any
    // DB query runs — same "validate first" position as
    // `validate_search_params`/`validate_municipality_slug`/
    // `validate_category` above and below.
    let sort_order = core::validate_sort(sort).map_err(|_| StatusCode::BAD_REQUEST)?;

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

    // IMP-REQ-009-06: `sqlx::query!`'s compile-time SQL checking requires a
    // string literal, so the `ORDER BY` clause can't be parameter-bound —
    // hence two textually near-identical branches (same `WHERE`/bind
    // params, different `ORDER BY`) rather than one query with an
    // interpolated column name. `SortOrder::Relevance` preserves the
    // pre-existing `civic_address_normalized ASC` ordering (TC-009-2
    // regression guard); `SortOrder::Date` orders by `latest_meeting_date
    // DESC NULLS LAST` (TC-009-1/-3), tie-broken by
    // `civic_address_normalized ASC` for determinism (TC-009-4).
    let rows = match sort_order {
        core::SortOrder::Relevance => sqlx::query!(
            r#"
            SELECT project_id, civic_address_normalized, municipality_name, project_type, normalized_status, source_language, latest_meeting_date, first_detected_at, source_count
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
            display_name: Some(core::synthesize_display_name(
                &row.civic_address_normalized,
                row.project_type.as_deref(),
            )),
            project_type: row.project_type,
            normalized_status: row.normalized_status,
            source_language: row.source_language,
            first_detected_at: row.first_detected_at,
            source_count: row.source_count,
            detection_sentence: None,
            latest_meeting_date: row.latest_meeting_date,
        })
        .collect(),
        core::SortOrder::Date => sqlx::query!(
            r#"
            SELECT project_id, civic_address_normalized, municipality_name, project_type, normalized_status, source_language, latest_meeting_date, first_detected_at, source_count
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
            ORDER BY latest_meeting_date DESC NULLS LAST, civic_address_normalized ASC
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
            display_name: Some(core::synthesize_display_name(
                &row.civic_address_normalized,
                row.project_type.as_deref(),
            )),
            project_type: row.project_type,
            normalized_status: row.normalized_status,
            source_language: row.source_language,
            first_detected_at: row.first_detected_at,
            source_count: row.source_count,
            detection_sentence: None,
            latest_meeting_date: row.latest_meeting_date,
        })
        .collect(),
    };

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
        RawResultFilterQuery {
            category: params.category,
            sort: params.sort,
        },
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
    // IMP-REQ-008-07/-10: the category chip row's accessible group label
    // (the `role="group"`'s `aria-label`) and the "All categories" chip's
    // own text (the default/clear-filter option, matching
    // `municipality_all_option`'s role for the municipality `<select>`).
    category_filter_label: &'static str,
    category_all_option: &'static str,
    // IMP-REQ-009-10: labels for the sort toggle button pair
    // (IMP-REQ-009-08). `sort_filter_label` is the toggle group's accessible
    // group label (the `role="group"`'s `aria-label`), mirroring
    // `category_filter_label`'s own role; `sort_relevance_label`/
    // `sort_date_label` are each toggle button's own visible text.
    sort_filter_label: &'static str,
    sort_relevance_label: &'static str,
    sort_date_label: &'static str,
    // IMP-REQ-009-09: label prefixing each result row's meeting-date display
    // (only rendered when `SearchResult.latest_meeting_date` is present).
    meeting_date_label: &'static str,
    // IMP-REQ-011-04: labels for the mobile filter sheet's trigger button,
    // panel title, and close control.
    filter_sheet_open_label: &'static str,
    filter_sheet_title: &'static str,
    filter_sheet_close_label: &'static str,
    // REQ-012: the richer zero-results empty state — a distinct headline
    // and body (rendered alongside, not replacing, the existing
    // `empty_message`/`empty_guidance` pair above), exactly four
    // refinement suggestions, and the two empty-state action links' own
    // visible text. The action links' `href`s ("/search", "/") are
    // language-independent literals, so they live directly in
    // `templates/empty_state.html` rather than as `SearchLabels` fields.
    empty_state_heading: &'static str,
    empty_state_body: &'static str,
    empty_state_suggestions: [&'static str; 4],
    empty_state_clear_filters_label: &'static str,
    empty_state_browse_all_label: &'static str,
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
            category_filter_label: "Filtrer par catégorie",
            category_all_option: "Toutes les catégories",
            sort_filter_label: "Trier par",
            sort_relevance_label: "Pertinence",
            sort_date_label: "Date de réunion",
            meeting_date_label: "Réunion :",
            filter_sheet_open_label: "Filtres",
            filter_sheet_title: "Filtres",
            filter_sheet_close_label: "Fermer les filtres",
            empty_state_heading: "Aucun résultat pour votre recherche",
            empty_state_body: "Essayez d'ajuster vos filtres ci-dessous, ou explorez ces suggestions.",
            empty_state_suggestions: [
                "Essayez une autre municipalité",
                "Essayez une autre période",
                "Essayez un terme de recherche plus large",
                "Vérifiez l'orthographe",
            ],
            empty_state_clear_filters_label: "Effacer les filtres",
            empty_state_browse_all_label: "Parcourir tous les projets",
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
            category_filter_label: "Filter by category",
            category_all_option: "All categories",
            sort_filter_label: "Sort results",
            sort_relevance_label: "Relevance",
            sort_date_label: "Meeting date",
            meeting_date_label: "Meeting:",
            filter_sheet_open_label: "Filters",
            filter_sheet_title: "Filters",
            filter_sheet_close_label: "Close filters",
            empty_state_heading: "No matches for your search",
            empty_state_body: "Try adjusting your filters below, or explore these suggestions.",
            empty_state_suggestions: [
                "Try another municipality",
                "Try a different date range",
                "Try a broader search term",
                "Check your spelling",
            ],
            empty_state_clear_filters_label: "Clear filters",
            empty_state_browse_all_label: "Browse all projects",
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
) -> Result<(StatusCode, HeaderMap, Html<String>), StatusCode> {
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

    // IMP-REQ-011-08: test-only fault-injection hook (TC-011-5). Lets a
    // test force `/search` to render its error state — through the same
    // responsive shell as every other page (TC-011-4) — without needing a
    // real DB outage (unlike `pool.close()`, which can't be scoped to a
    // single request/test alongside other assertions in the same suite).
    // Gated on `cfg(debug_assertions)` so it compiles out of release
    // builds entirely: a production deployment (built with `--release`)
    // never even contains the code path that reads these signals, so it
    // can't be used to force an outage against a live deployment. Mirrors
    // `AppState::citation_db_override`'s existing "test-only hook" pattern
    // for the project-detail page, just via a compile-time gate instead of
    // a runtime `Option` field (no new `AppState` field needed here, since
    // the signal is a per-request header/query param, not a fixture the
    // test harness wires in once at `AppState` construction).
    #[cfg(debug_assertions)]
    let fault_forced = core::should_force_fault(
        params.force_fault.as_deref(),
        headers.get("x-force-fault").and_then(|v| v.to_str().ok()),
    );
    #[cfg(not(debug_assertions))]
    let fault_forced = false;

    if fault_forced {
        let tmpl = state
            .env
            .get_template("search.html")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let html = tmpl
            .render(context! {
                lang => lang,
                nav_permits => labels.nav_permits,
                nav_council => labels.nav_council,
                page_title => labels.page_title,
                heading => labels.heading,
                search_label => labels.search_label,
                submit_label => labels.submit_label,
                query => params.q,
                has_searched => true,
                search_error => true,
                lang_toggle_href => core::build_lang_toggle_href(
                    lang,
                    &params.q,
                    params.municipality_slug.as_deref(),
                ),
                lang_toggle_label => labels.lang_toggle_label,
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok((StatusCode::SERVICE_UNAVAILABLE, HeaderMap::new(), Html(html)));
    }

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
                RawResultFilterQuery {
                    category: params.category.clone(),
                    sort: params.sort.clone(),
                },
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
    let (mut search_results, search_error, pagination) = match search_outcome {
        Some(Ok((results, pagination))) => (results, false, Some(pagination)),
        Some(Err(_)) => (Vec::new(), true, None),
        None => (Vec::new(), false, None),
    };

    // IMP-REQ-015-06/-11: derive each result's "Detected N day(s) ago from M
    // council source(s)" sentence here in the shell, where `lang` and a
    // single shared `now` are both known — `run_search` itself has neither,
    // by design (IMP-REQ-015-03's pure function takes both as arguments
    // rather than reading a clock).
    let detection_now = chrono::Utc::now();
    for result in &mut search_results {
        result.detection_sentence = core::format_detection_sentence(
            lang,
            result.first_detected_at,
            result.source_count,
            detection_now,
        );
    }

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

    // IMP-REQ-008-08: populates the category chip row from the live
    // `category_taxonomy` table (`is_public`, `sort_order`), the same query
    // `list_categories` (`GET /categories`) runs. A query failure here
    // degrades to an empty list (`category_chips` stays empty, so the whole
    // chip row is omitted by the template's `{% if category_chips %}` guard)
    // rather than failing the whole page render — mirroring the
    // `municipalities` select's own degradation above.
    let category_codes: Vec<String> = sqlx::query_scalar!(
        "SELECT code FROM category_taxonomy WHERE is_public ORDER BY sort_order"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // IMP-REQ-008-07: the raw `category` query param, trimmed and
    // lowercased for chip-selection comparison — mirrors
    // `core::validate_category`'s own normalization so a chip renders
    // selected/`aria-current` for exactly the same value `run_search` above
    // actually filtered on.
    let selected_category = params
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let filter_href_ctx = core::FilterHrefContext {
        lang,
        q: &params.q,
        municipality_slug: params.municipality_slug.as_deref(),
        date_preset: params.date_preset.as_deref(),
        date_from: params.date_from.as_deref(),
        date_to: params.date_to.as_deref(),
        sort: params.sort.as_deref(),
        category: params.category.as_deref(),
    };
    let category_href_ctx = filter_href_ctx;
    let category_all_href = core::build_category_filter_href(category_href_ctx, None);
    let category_chips: Vec<CategoryChip> = category_codes
        .into_iter()
        .map(|code| {
            let selected = selected_category.as_deref() == Some(code.as_str());
            let label = core::category_display_name(&code, lang)
                .map(str::to_string)
                .unwrap_or_else(|| code.clone());
            let href = core::build_category_filter_href(category_href_ctx, Some(&code));
            CategoryChip {
                code,
                label,
                href,
                selected,
            }
        })
        .collect();

    // IMP-REQ-003-08: the toggle always targets the OTHER language than
    // this page's resolved `lang`, preserving the `q`/`municipality_slug`
    // filters actually in play so following the link re-renders the same
    // search rather than losing the user's current filters.
    let lang_toggle_href =
        core::build_lang_toggle_href(lang, &params.q, params.municipality_slug.as_deref());

    // IMP-REQ-009-07/-08: `active_sort` drives the toggle button pair's
    // `aria-current`/visual-selected styling (IMP-REQ-009-11: never
    // color-only), matching `selected_category`'s own role for the category
    // chip row. Falls back to `SortOrder::Relevance` for a malformed `sort`
    // value (`run_search` above already surfaced that as `search_error` —
    // this is purely about which toggle link renders "active" in the
    // chrome, not a second validation pass).
    let active_sort = core::validate_sort(params.sort.clone()).unwrap_or(core::SortOrder::Relevance);
    let active_sort_label = match active_sort {
        core::SortOrder::Date => "date",
        core::SortOrder::Relevance => "relevance",
    };
    let sort_relevance_href =
        core::build_sort_toggle_href(filter_href_ctx, core::SortOrder::Relevance);
    let sort_date_href = core::build_sort_toggle_href(filter_href_ctx, core::SortOrder::Date);

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
            category_filter_label => labels.category_filter_label,
            category_all_option => labels.category_all_option,
            category_all_href => category_all_href,
            selected_category => selected_category,
            category_chips => category_chips,
            sort_filter_label => labels.sort_filter_label,
            sort_relevance_label => labels.sort_relevance_label,
            sort_date_label => labels.sort_date_label,
            sort_relevance_href => sort_relevance_href,
            sort_date_href => sort_date_href,
            active_sort => active_sort_label,
            meeting_date_label => labels.meeting_date_label,
            filter_sheet_open_label => labels.filter_sheet_open_label,
            filter_sheet_title => labels.filter_sheet_title,
            filter_sheet_close_label => labels.filter_sheet_close_label,
            empty_state_heading => labels.empty_state_heading,
            empty_state_body => labels.empty_state_body,
            empty_state_suggestions => labels.empty_state_suggestions,
            empty_state_clear_filters_label => labels.empty_state_clear_filters_label,
            empty_state_browse_all_label => labels.empty_state_browse_all_label,
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

    Ok((StatusCode::OK, response_headers, Html(html)))
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

        let pairs: [(&str, &str, &str); 35] = [
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
            (
                "sort_filter_label",
                en.sort_filter_label,
                fr.sort_filter_label,
            ),
            (
                "sort_relevance_label",
                en.sort_relevance_label,
                fr.sort_relevance_label,
            ),
            ("sort_date_label", en.sort_date_label, fr.sort_date_label),
            (
                "meeting_date_label",
                en.meeting_date_label,
                fr.meeting_date_label,
            ),
            (
                "filter_sheet_open_label",
                en.filter_sheet_open_label,
                fr.filter_sheet_open_label,
            ),
            (
                "filter_sheet_title",
                en.filter_sheet_title,
                fr.filter_sheet_title,
            ),
            (
                "filter_sheet_close_label",
                en.filter_sheet_close_label,
                fr.filter_sheet_close_label,
            ),
            (
                "empty_state_heading",
                en.empty_state_heading,
                fr.empty_state_heading,
            ),
            (
                "empty_state_body",
                en.empty_state_body,
                fr.empty_state_body,
            ),
            (
                "empty_state_suggestions[0]",
                en.empty_state_suggestions[0],
                fr.empty_state_suggestions[0],
            ),
            (
                "empty_state_suggestions[1]",
                en.empty_state_suggestions[1],
                fr.empty_state_suggestions[1],
            ),
            (
                "empty_state_suggestions[2]",
                en.empty_state_suggestions[2],
                fr.empty_state_suggestions[2],
            ),
            (
                "empty_state_suggestions[3]",
                en.empty_state_suggestions[3],
                fr.empty_state_suggestions[3],
            ),
            (
                "empty_state_clear_filters_label",
                en.empty_state_clear_filters_label,
                fr.empty_state_clear_filters_label,
            ),
            (
                "empty_state_browse_all_label",
                en.empty_state_browse_all_label,
                fr.empty_state_browse_all_label,
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
