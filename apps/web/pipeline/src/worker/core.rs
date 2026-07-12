use scraper::{Html, Selector};

/// Extracts absolute URLs for `typeDoc=pv` (procès-verbal/minutes) links
/// from `html`, resolving relative hrefs against `base_url`. Ignores
/// `typeDoc=odj` (agenda, pre-decision) and `typeDoc=da` (attachment) links
/// — only minutes carry the recorded decision text
/// (`approval_status_raw`) this product surfaces (see the design doc's
/// Non-goals). Returns an empty `Vec` if `base_url` itself doesn't parse or
/// no matching links are found — never panics on malformed input.
pub(crate) fn extract_pv_document_links(html: &str, base_url: &str) -> Vec<String> {
    let Ok(base) = reqwest::Url::parse(base_url) else {
        return Vec::new();
    };

    let document = Html::parse_document(html);
    // Safe to unwrap: this selector is a fixed, valid CSS string.
    let selector = Selector::parse("a[href]").unwrap();

    document
        .select(&selector)
        .filter_map(|el| el.value().attr("href"))
        .filter(|href| href.to_lowercase().contains("typedoc=pv"))
        .filter_map(|href| base.join(href).ok())
        .map(|url| url.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_URL: &str =
        "https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL";

    fn real_fixture_html() -> String {
        let bytes = std::fs::read("tests/fixtures/montreal_listing_page.html")
            .expect("fixture file must exist — see Task 4 Step 1");
        // The real page is windows-1252-encoded; lossy UTF-8 decoding
        // mangles accented characters but leaves the pure-ASCII href
        // attributes (what this function reads) untouched.
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn extracts_exactly_the_known_pv_links_from_the_real_fixture() {
        let html = real_fixture_html();
        let links = extract_pv_document_links(&html, BASE_URL);

        let expected_doc_ids = ["8262", "8294", "8295", "8329", "8354", "8378", "8423"];
        assert_eq!(links.len(), expected_doc_ids.len());
        for doc_id in expected_doc_ids {
            let expected_url = format!(
                "https://ville.montreal.qc.ca/sel/adi-public/afficherpdf/fichier.pdf?typeDoc=pv&doc={doc_id}"
            );
            assert!(
                links.contains(&expected_url),
                "expected {expected_url} in {links:?}"
            );
        }
    }

    #[test]
    fn ignores_odj_and_da_links() {
        let html = real_fixture_html();
        let links = extract_pv_document_links(&html, BASE_URL);
        assert!(links.iter().all(|l| l.contains("typeDoc=pv")));
        assert!(!links.iter().any(|l| l.contains("typeDoc=odj")));
        assert!(!links.iter().any(|l| l.contains("typeDoc=da")));
    }

    #[test]
    fn returns_empty_for_html_with_no_links() {
        let links = extract_pv_document_links("<html><body>no links here</body></html>", BASE_URL);
        assert!(links.is_empty());
    }

    #[test]
    fn returns_empty_for_unparseable_base_url() {
        let html = r#"<a href="/sel/adi-public/afficherpdf/fichier.pdf?typeDoc=pv&doc=1">PV</a>"#;
        let links = extract_pv_document_links(html, "not a url");
        assert!(links.is_empty());
    }
}
