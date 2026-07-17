//! Shared UI-locale resolution utility (IMP-REQ-005-04).
//!
//! Originally implemented only inside `search.rs`'s `core` submodule
//! (IMP-REQ-003-03/-04) and used solely by `get_search_page`. Extracted here
//! so `projects::get_project_detail_page` can share the exact same
//! `?lang=` param > `lang` cookie > `Accept-Language` header > `"en"`
//! default precedence (IMP-REQ-005-04) instead of duplicating it or falling
//! back to the plainer `routes::detect_lang` (which only ever consults
//! `Accept-Language`).
//!
//! `normalize_query`/`validate_search_params`/`validate_municipality_slug`/etc.
//! stay in `search.rs`'s own `core` module — those are genuinely
//! search-specific and have no bearing on locale resolution.

use axum::http::HeaderMap;

/// Parses a single lang value (an explicit `?lang=` param or a `lang`
/// cookie value, already extracted from the `Cookie` header by the caller)
/// into a recognized `"fr"`/`"en"` tag. Case-insensitive on the exact
/// two-letter code; anything else (garbage like `"xx"`, empty string, a full
/// BCP-47 tag) is treated as unrecognized rather than guessed at, so the
/// caller falls through to the next precedence level instead of
/// misinterpreting it.
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
/// `routes::detect_lang` does (first comma-separated tag, ignoring `;q=`
/// weights, prefix-matched against `fr`/`en`), but as a pure function over an
/// already-extracted `&str` rather than a `HeaderMap`, so it can be
/// unit-tested and reused from `resolve_ui_locale` without an HTTP request in
/// hand. Defaults to `"en"` when nothing recognized is found, matching
/// `detect_lang`'s existing behavior.
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

/// Resolves a page's UI rendering locale (IMP-REQ-003-03, shared via
/// IMP-REQ-005-04) from the three sources REQ-003 commits to, in precedence
/// order: an explicit `?lang=` query param, then a `lang` cookie, then the
/// `Accept-Language` header, then `"en"` as the final default.
///
/// Each source is tried in turn via `parse_lang_tag`
/// (`explicit_param`/`cookie`) or `parse_accept_language`
/// (`accept_language_header`); a source that is absent (`None`) or present
/// but unrecognized (e.g. an `xx` cookie) does not short-circuit to the
/// default — it simply falls through to the next source in the precedence
/// chain, so a garbage explicit param still lets a valid cookie or header
/// win, and only exhausting all three falls back to `"en"`.
///
/// Callers pass already-extracted values: `cookie` is the `lang` cookie's
/// value (not the raw `Cookie` header), and `accept_language_header` is the
/// raw `Accept-Language` header value (not a parsed tag) since it needs
/// `parse_accept_language`'s comma-list handling internally.
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

/// Extracts the `lang` cookie's value from a raw `Cookie` request header
/// (IMP-REQ-003-04). `Cookie` headers are a single `; `-separated list of
/// `name=value` pairs (RFC 6265 §5.4) — this is a minimal parser scoped to
/// finding one specific cookie by name, not a general cookie-jar
/// implementation, since that's all `resolve_ui_locale` needs.
pub fn extract_cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// An invalid/garbage explicit param (e.g. `xx`) does not win by virtue
    /// of being present — it's unrecognized, so resolution falls through to
    /// the cookie.
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

    /// When every source is either absent or unrecognized, resolution still
    /// falls back to the `"en"` default rather than panicking.
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
