use scraper::{Html, Selector};

use super::{ParseMethod, ParsedChunk};

const BOILERPLATE_TAGS: &[&str] = &[
    "nav", "header", "footer", "aside", "script", "style", "noscript",
];

/// Extracts ordered text chunks from block-level elements (paragraphs, list
/// items, headings, table cells), skipping anything nested under a
/// boilerplate container (`nav`/`header`/`footer`/`aside`/`script`/`style`).
pub fn parse(body: &[u8]) -> Vec<ParsedChunk> {
    let text = String::from_utf8_lossy(body);
    let document = Html::parse_document(&text);
    // Safe to unwrap: this selector is a fixed, valid CSS string.
    let selector = Selector::parse("p, li, h1, h2, h3, h4, h5, h6, td").unwrap();

    document
        .select(&selector)
        .filter(|element| !is_in_boilerplate(element))
        .filter_map(|element| {
            let content = element
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if content.is_empty() {
                None
            } else {
                Some(ParsedChunk {
                    content,
                    parse_method: ParseMethod::Text,
                })
            }
        })
        .collect()
}

fn is_in_boilerplate(element: &scraper::ElementRef) -> bool {
    element.ancestors().any(|node| {
        node.value()
            .as_element()
            .is_some_and(|el| BOILERPLATE_TAGS.contains(&el.name()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-REQ-002-1 (HTML half): fixture parses into correctly ordered chunks.
    #[test]
    fn parse_extracts_ordered_paragraphs() {
        let html = b"<html><body><p>First</p><p>Second</p><p>Third</p></body></html>";
        let chunks = parse(html);
        let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        assert_eq!(contents, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn parse_removes_boilerplate_navigation_and_footer() {
        let html = b"<html><body>\
            <nav><p>Skip to content</p></nav>\
            <header><p>Site Header</p></header>\
            <p>Actual agenda item text</p>\
            <footer><p>Copyright 2026</p></footer>\
            </body></html>";
        let chunks = parse(html);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Actual agenda item text");
    }

    #[test]
    fn parse_skips_script_and_style_content() {
        let html = b"<html><body>\
            <script>var x = 1;</script>\
            <style>.foo { color: red; }</style>\
            <p>Real content</p>\
            </body></html>";
        let chunks = parse(html);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Real content");
    }

    #[test]
    fn parse_empty_document_yields_no_chunks() {
        let chunks = parse(b"<html><body></body></html>");
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_collapses_internal_whitespace() {
        let html = b"<html><body><p>  extra   \n  whitespace  </p></body></html>";
        let chunks = parse(html);
        assert_eq!(chunks[0].content, "extra whitespace");
    }
}
