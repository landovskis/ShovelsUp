/// Deterministic address normalization for exact-match project resolution
/// (TC-REQ-005-2 requires near-miss addresses to NOT auto-link — this is
/// exact-string matching after normalization, not fuzzy similarity).
///
/// This is the English-only baseline. REQ-007 extends this module with
/// French-Quebec-specific rules (accented street-type abbreviations, "boul."
/// / "av." / "rue", etc.) — matcher logic in this module (`address_type_match`)
/// stays language-agnostic and shared, per REQ-007's Implementation Strategy.
pub fn normalize_address(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    let mut normalized = lower;
    for (abbrev, expanded) in ABBREVIATIONS {
        // Word-boundary replace: only at the end of a word (preceded by a
        // space or start of string) and followed by end-of-string, comma,
        // period, or space — avoids matching "st" inside "street" itself.
        normalized = replace_word(&normalized, abbrev, expanded);
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

const ABBREVIATIONS: &[(&str, &str)] = &[
    ("st.", "street"),
    ("st", "street"),
    ("ave.", "avenue"),
    ("ave", "avenue"),
    ("rd.", "road"),
    ("rd", "road"),
    ("blvd.", "boulevard"),
    ("blvd", "boulevard"),
    ("dr.", "drive"),
    ("dr", "drive"),
    ("ct.", "court"),
    ("ct", "court"),
    ("pkwy.", "parkway"),
    ("pkwy", "parkway"),
    ("ln.", "lane"),
    ("ln", "lane"),
];

fn replace_word(text: &str, word: &str, replacement: &str) -> String {
    text.split(' ')
        .map(|token| {
            let trimmed = token.trim_end_matches(['.', ',']);
            if trimmed == word.trim_end_matches('.') {
                replacement
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_whitespace() {
        assert_eq!(normalize_address("  123   MAIN   St  "), "123 main street");
    }

    #[test]
    fn expands_common_abbreviations() {
        assert_eq!(normalize_address("123 Main St"), "123 main street");
        assert_eq!(normalize_address("456 Oak Ave"), "456 oak avenue");
        assert_eq!(normalize_address("789 Elm Rd"), "789 elm road");
    }

    #[test]
    fn different_addresses_normalize_differently() {
        // TC-REQ-005-2: near-miss addresses must not collide.
        assert_ne!(
            normalize_address("123 Main St"),
            normalize_address("125 Main St")
        );
        assert_ne!(
            normalize_address("123 Main St"),
            normalize_address("123 Maine St")
        );
    }

    #[test]
    fn equivalent_phrasing_normalizes_the_same() {
        assert_eq!(
            normalize_address("123 Main St."),
            normalize_address("123 Main Street")
        );
    }
}
