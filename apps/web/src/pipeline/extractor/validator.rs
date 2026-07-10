/// Deterministic RULE-001 (physical-work filter) override. The
/// Implementation Strategy for REQ-003 is explicit that LLM self-reporting
/// of `physical_work` cannot be trusted, so this function re-derives the
/// classification from the source text and overrides the LLM's claim
/// whenever the text is unambiguous — the LLM's classification is used only
/// when neither keyword set matches (genuinely ambiguous text).
const REZONING_ONLY_KEYWORDS: &[&str] = &[
    "rezoning",
    "zoning amendment",
    "zoning by-law amendment",
    "official plan amendment",
    "change in land use designation",
];

const PHYSICAL_WORK_KEYWORDS: &[&str] = &[
    "demolition",
    "demolish",
    "construction of",
    "erect",
    "erection of",
    "building permit",
    "new building",
    "addition to",
    "renovation",
    "excavation",
    "expansion of",
    "conversion of",
];

pub fn validate_physical_work(chunk_text: &str, llm_claimed: bool) -> bool {
    let lower = chunk_text.to_lowercase();
    let has_physical = PHYSICAL_WORK_KEYWORDS.iter().any(|kw| lower.contains(kw));
    let has_rezoning_only = REZONING_ONLY_KEYWORDS.iter().any(|kw| lower.contains(kw));

    if has_rezoning_only && !has_physical {
        // Administrative/rezoning-only language with no physical-work
        // language present overrides any LLM claim to the contrary.
        false
    } else if has_physical {
        true
    } else {
        llm_claimed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-REQ-003-3: rezoning-only motion excluded despite LLM hallucination.
    #[test]
    fn overrides_llm_true_for_rezoning_only_text() {
        let text = "Item 12: Zoning by-law amendment to permit mixed-use designation at 400 King St.";
        assert!(!validate_physical_work(text, true));
    }

    #[test]
    fn overrides_llm_false_for_clear_physical_work_text() {
        let text = "Application for demolition of the existing structure at 12 Elm St to permit construction of a new 6-storey building.";
        assert!(validate_physical_work(text, false));
    }

    #[test]
    fn trusts_llm_for_ambiguous_text() {
        let text = "Item 3 was discussed and referred to staff for further review.";
        assert!(!validate_physical_work(text, false));
        assert!(validate_physical_work(text, true));
    }

    #[test]
    fn physical_work_keyword_wins_over_rezoning_keyword_when_both_present() {
        let text = "Rezoning approved to permit construction of a new residential building.";
        assert!(validate_physical_work(text, false));
    }
}
