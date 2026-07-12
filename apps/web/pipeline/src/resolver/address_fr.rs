/// French-Quebec address normalization (IMP-REQ-007-02), a sibling of
/// `address::normalize_address` for the same exact-match resolution use
/// (TC-REQ-005-1/-2 near-miss addresses must not collide, applied
/// identically here). The address-type matcher logic itself
/// (`address_type_match` in `resolver::mod`) stays language-agnostic and
/// shared, per REQ-007's Implementation Strategy — only the normalization
/// ruleset differs by language.
///
/// Handles the Quebec-specific conventions this baseline English normalizer
/// does not: leading article/street-type ordering ("123, rue Principale"
/// rather than "123 Main St"), accented street types, and French
/// abbreviations (boul., av., ch., etc.).
pub fn normalize_address_fr(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    // Quebec civic addresses commonly separate the number from the street
    // name with a comma ("123, rue Principale") — normalize that to a
    // single space so it matches comma-free variants of the same address.
    let mut normalized = lower.replace(',', " ");
    for (abbrev, expanded) in ABBREVIATIONS {
        normalized = replace_word(&normalized, abbrev, expanded);
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

const ABBREVIATIONS: &[(&str, &str)] = &[
    ("boul.", "boulevard"),
    ("boul", "boulevard"),
    ("blvd.", "boulevard"),
    ("blvd", "boulevard"),
    ("av.", "avenue"),
    ("ave.", "avenue"),
    ("ave", "avenue"),
    ("ch.", "chemin"),
    ("ch", "chemin"),
    ("rte.", "route"),
    ("rte", "route"),
    ("pl.", "place"),
    ("pl", "place"),
    ("terr.", "terrasse"),
    ("terr", "terrasse"),
    ("mtée.", "montée"),
    ("mtée", "montée"),
    ("c.p.", "case postale"),
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

    /// IMP-REQ-007-02 acceptance criteria: real Montreal addresses
    /// normalize to a canonical form, covering the comma-separated civic
    /// number, accented street types, and common French abbreviations.
    #[test]
    fn normalizes_real_montreal_addresses_to_canonical_form() {
        let cases: &[(&str, &str)] = &[
            ("123, rue Principale", "123 rue principale"),
            ("123 rue Principale", "123 rue principale"),
            ("456, boul. Saint-Laurent", "456 boulevard saint-laurent"),
            ("456 Boulevard Saint-Laurent", "456 boulevard saint-laurent"),
            ("789, av. du Parc", "789 avenue du parc"),
            ("789 Avenue du Parc", "789 avenue du parc"),
            (
                "1000, ch. de la Côte-des-Neiges",
                "1000 chemin de la côte-des-neiges",
            ),
            (
                "1000 Chemin de la Côte-des-Neiges",
                "1000 chemin de la côte-des-neiges",
            ),
            (
                "200, rue Sainte-Catherine Ouest",
                "200 rue sainte-catherine ouest",
            ),
            ("15, rue Saint-Denis", "15 rue saint-denis"),
            ("50, boul. René-Lévesque", "50 boulevard rené-lévesque"),
            ("50 Boulevard René-Lévesque", "50 boulevard rené-lévesque"),
            ("300, av. Mont-Royal", "300 avenue mont-royal"),
            ("300 Avenue Mont-Royal", "300 avenue mont-royal"),
            ("25, rue Sherbrooke Est", "25 rue sherbrooke est"),
            ("18, ch. Queen-Mary", "18 chemin queen-mary"),
            ("400, boul. de Maisonneuve", "400 boulevard de maisonneuve"),
            (
                "400 Boulevard de Maisonneuve",
                "400 boulevard de maisonneuve",
            ),
            ("60, rue Ontario Est", "60 rue ontario est"),
            ("5, place Ville-Marie", "5 place ville-marie"),
            ("5, pl. Ville-Marie", "5 place ville-marie"),
            ("10, rte. Transcanadienne", "10 route transcanadienne"),
        ];

        for (input, expected) in cases {
            assert_eq!(normalize_address_fr(input), *expected, "input: {input}");
        }
    }

    #[test]
    fn different_addresses_normalize_differently() {
        // Mirrors TC-REQ-005-2 for the French normalizer: near-miss
        // addresses must not collide into the same normalized string.
        assert_ne!(
            normalize_address_fr("123, rue Principale"),
            normalize_address_fr("125, rue Principale")
        );
        assert_ne!(
            normalize_address_fr("123, rue Principale"),
            normalize_address_fr("123, rue Principe")
        );
    }

    #[test]
    fn equivalent_phrasing_normalizes_the_same() {
        assert_eq!(
            normalize_address_fr("456, boul. Saint-Laurent"),
            normalize_address_fr("456 Boulevard Saint-Laurent")
        );
    }
}
