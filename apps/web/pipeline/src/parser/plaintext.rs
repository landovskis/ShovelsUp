use super::{ParseMethod, ParsedChunk};

/// Decodes `body` as UTF-8, falling back to Windows-1252 (a practical
/// superset of Latin-1/ISO-8859-1 for the printable range, and what
/// `encoding_rs` exposes — plain ISO-8859-1 isn't a distinct decoder in that
/// crate) when the bytes aren't valid UTF-8. Splits on blank lines into
/// ordered paragraph chunks; an empty (or all-whitespace) document yields
/// zero chunks without error (TC-REQ-002-2).
pub fn parse(body: &[u8]) -> Vec<ParsedChunk> {
    let text = match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (decoded, _encoding, _had_errors) = encoding_rs::WINDOWS_1252.decode(body);
            decoded.into_owned()
        }
    };

    text.split("\n\n")
        .map(|paragraph| paragraph.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|paragraph| !paragraph.is_empty())
        .map(|content| ParsedChunk {
            content,
            parse_method: ParseMethod::Text,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-REQ-002-2: empty document produces zero chunks without error.
    #[test]
    fn parse_empty_document_yields_no_chunks() {
        assert!(parse(b"").is_empty());
        assert!(parse(b"   \n\n  ").is_empty());
    }

    #[test]
    fn parse_utf8_splits_on_blank_lines() {
        let chunks = parse("First paragraph.\n\nSecond paragraph.".as_bytes());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].content, "First paragraph.");
        assert_eq!(chunks[1].content, "Second paragraph.");
    }

    /// A Latin-1-encoded French fixture (e.g. "d\xe9cision" = "décision")
    /// must decode without corruption or replacement characters.
    #[test]
    fn parse_falls_back_to_latin1_for_non_utf8_french_text() {
        // "Décision municipale" with an é encoded as Latin-1 0xE9 (invalid
        // as a UTF-8 continuation byte on its own, so this is not valid UTF-8).
        let mut bytes = b"D\xe9cision municipale".to_vec();
        assert!(std::str::from_utf8(&bytes).is_err());
        bytes.push(b'.');

        let chunks = parse(&bytes);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Décision municipale.");
    }
}
