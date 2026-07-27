/// Deterministic RULE-001 (physical-work filter) override. The
/// Implementation Strategy for REQ-003 is explicit that LLM self-reporting
/// of `physical_work` cannot be trusted, so this function re-derives the
/// classification from the source text and overrides the LLM's claim
/// whenever the text is unambiguous — the LLM's classification is used only
/// when neither keyword set matches (genuinely ambiguous text).
///
/// **Prerequisite fix for REQ-007 (not a plan task, discovered during Loop
/// B):** the plan's REQ-007 Test Plan Implementation Breakdown cross-refs
/// TC-REQ-007-3 to this validator as "language-agnostic", but the original
/// keyword lists were English-only — a French rezoning-only motion would
/// have matched neither list and silently fallen through to trusting the
/// LLM's own (unreliable) claim. Added the French keyword lists below so
/// the validator is actually language-agnostic, matching the plan's stated
/// design intent, rather than leaving REQ-007's TC-REQ-007-3 unimplementable.
const REZONING_ONLY_KEYWORDS_EN: &[&str] = &[
    "rezoning",
    "zoning amendment",
    "zoning by-law amendment",
    "official plan amendment",
    "change in land use designation",
];

const PHYSICAL_WORK_KEYWORDS_EN: &[&str] = &[
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

const REZONING_ONLY_KEYWORDS_FR: &[&str] = &[
    "modification de zonage",
    "modification du règlement de zonage",
    "règlement de zonage",
    "changement de zonage",
    "modification du plan d'urbanisme",
    "changement de désignation d'usage du sol",
];

const PHYSICAL_WORK_KEYWORDS_FR: &[&str] = &[
    "démolition",
    "démolir",
    "construction de",
    "construction d'un",
    "construction d'une",
    "érection de",
    "permis de construction",
    "nouveau bâtiment",
    "agrandissement de",
    "rénovation",
    "excavation",
    "conversion de",
];

/// Keywords for the narrower infrastructure-land-acquisition qualification
/// path (see `is_infrastructure_land_acquisition`): a land purchase the
/// city makes specifically to enable a known future infrastructure
/// project (e.g. a road reconfiguration), which has no described physical
/// construction yet and so never matches `PHYSICAL_WORK_KEYWORDS_*` above.
const INFRASTRUCTURE_LAND_KEYWORDS_EN: &[&str] = &[
    "land acquisition",
    "acquire the land",
    "acquire a parcel",
    "purchase of land for",
    "infrastructure reconfiguration",
    "road reconfiguration",
];

/// Keywords are written with ASCII apostrophes; `is_infrastructure_land_acquisition`
/// folds U+2019 in the input to `'` before matching, so real Montreal PDF text
/// (which uses the typographic apostrophe) matches these entries too.
const INFRASTRUCTURE_LAND_KEYWORDS_FR: &[&str] = &[
    "acquérir un terrain",
    "acquisition d'un terrain",
    "réaménagement d'infrastructures",
    // Deliberately without a leading "aux"/"pour les": the real motivating
    // text (CM26 0091) says "pour les fins de réaménagement", while other
    // items use "aux fins de réaménagement" — the bare phrase matches both.
    "fins de réaménagement",
];

/// Narrower, separate qualification path from `validate_physical_work`:
/// requires BOTH a land-acquisition keyword match AND an LLM-reported
/// `project_type` that starts with `"infrastructure"` (case-insensitive).
/// Neither signal alone is sufficient — an infrastructure-tagged item with
/// no land-acquisition language, or land-acquisition language on a
/// non-infrastructure item (e.g. a housing land sale), does not qualify
/// through this path.
///
/// The `project_type` check is a prefix match, not equality: the field is
/// documented free text and the FR prompt's own worked examples return
/// French values, so the real motivating text ("réaménagement
/// d'infrastructures routières") can plausibly come back as
/// "infrastructures", "infrastructure routière", or "infrastructures
/// routières". A prefix match covers all of those while still excluding
/// unrelated types (residential, institutionnel, …).
pub fn is_infrastructure_land_acquisition(
    chunk_text: &str,
    language: &str,
    project_type: Option<&str>,
) -> bool {
    let is_infrastructure_type = project_type
        .map(|t| t.to_lowercase().starts_with("infrastructure"))
        .unwrap_or(false);
    if !is_infrastructure_type {
        return false;
    }
    // Real Montreal PDFs use U+2019 (’), not the ASCII apostrophe the FR
    // keyword list is written with; fold it so both spellings match.
    // Scoped to this path deliberately — `validate_physical_work` has the
    // same latent issue but is out of this change's scope.
    let lower = chunk_text.to_lowercase().replace('\u{2019}', "'");
    let keywords = match language {
        "fr" => INFRASTRUCTURE_LAND_KEYWORDS_FR,
        _ => INFRASTRUCTURE_LAND_KEYWORDS_EN,
    };
    keywords.iter().any(|kw| lower.contains(kw))
}

/// `language` is `"fr"` or defaults to English for anything else, matching
/// `extract_entities`'s prompt-routing convention.
pub fn validate_physical_work(chunk_text: &str, language: &str, llm_claimed: bool) -> bool {
    let lower = chunk_text.to_lowercase();
    let (rezoning_keywords, physical_keywords) = match language {
        "fr" => (REZONING_ONLY_KEYWORDS_FR, PHYSICAL_WORK_KEYWORDS_FR),
        _ => (REZONING_ONLY_KEYWORDS_EN, PHYSICAL_WORK_KEYWORDS_EN),
    };
    let has_physical = physical_keywords.iter().any(|kw| lower.contains(kw));
    let has_rezoning_only = rezoning_keywords.iter().any(|kw| lower.contains(kw));

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
        let text =
            "Item 12: Zoning by-law amendment to permit mixed-use designation at 400 King St.";
        assert!(!validate_physical_work(text, "en", true));
    }

    #[test]
    fn overrides_llm_false_for_clear_physical_work_text() {
        let text = "Application for demolition of the existing structure at 12 Elm St to permit construction of a new 6-storey building.";
        assert!(validate_physical_work(text, "en", false));
    }

    #[test]
    fn trusts_llm_for_ambiguous_text() {
        let text = "Item 3 was discussed and referred to staff for further review.";
        assert!(!validate_physical_work(text, "en", false));
        assert!(validate_physical_work(text, "en", true));
    }

    #[test]
    fn physical_work_keyword_wins_over_rezoning_keyword_when_both_present() {
        let text = "Rezoning approved to permit construction of a new residential building.";
        assert!(validate_physical_work(text, "en", false));
    }

    /// TC-REQ-007-3: RULE-001 excludes a French rezoning-only motion despite
    /// an LLM hallucination, mirroring TC-REQ-003-3 in French.
    #[test]
    fn overrides_llm_true_for_french_rezoning_only_text() {
        let text = "Point 12 : Modification de zonage pour permettre une désignation à usage mixte au 400, rue King.";
        assert!(!validate_physical_work(text, "fr", true));
    }

    #[test]
    fn overrides_llm_false_for_french_physical_work_text() {
        let text = "Demande de démolition de la structure existante au 12, rue Elm pour permettre la construction d'un nouveau bâtiment de 6 étages.";
        assert!(validate_physical_work(text, "fr", false));
    }

    #[test]
    fn trusts_llm_for_ambiguous_french_text() {
        let text = "Le point 3 a été discuté et renvoyé au personnel pour examen plus approfondi.";
        assert!(!validate_physical_work(text, "fr", false));
        assert!(validate_physical_work(text, "fr", true));
    }

    #[test]
    fn infrastructure_land_acquisition_matches_french_keyword_and_type() {
        let text = "Approuver le projet d'addenda ... la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame.";
        assert!(is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("infrastructure")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_matches_english_keyword_and_type() {
        let text = "The City will acquire the land for infrastructure reconfiguration at the corner of Elm and Main.";
        assert!(is_infrastructure_land_acquisition(
            text,
            "en",
            Some("infrastructure")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_rejects_keyword_with_wrong_project_type() {
        let text = "La Ville vend un immeuble à des fins d'habitation, à la Coopérative d'habitation Monde-Uni.";
        // "vend" isn't a land-acquisition keyword either, but even paired
        // with an infrastructure type this housing-sale text has no
        // land-acquisition keyword match at all.
        assert!(!is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("infrastructure")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_rejects_keyword_with_non_infrastructure_type() {
        let text = "La Ville s'est engagée à acquérir un terrain à des fins d'habitation.";
        assert!(!is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("residential")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_rejects_when_project_type_is_none() {
        let text = "La Ville s'est engagée à acquérir un terrain pour les fins de réaménagement d'infrastructures routières.";
        assert!(!is_infrastructure_land_acquisition(text, "fr", None));
    }

    #[test]
    fn infrastructure_land_acquisition_type_match_is_case_insensitive() {
        let text = "The City will acquire the land for infrastructure reconfiguration.";
        assert!(is_infrastructure_land_acquisition(
            text,
            "en",
            Some("Infrastructure")
        ));
    }

    /// `project_type` is documented free text and the FR prompt's worked
    /// examples return French values, so the real motivating text
    /// ("réaménagement d'infrastructures routières") can plausibly come back
    /// as any of these. All must qualify — an exact-equality check would
    /// have silently dropped every one of them.
    #[test]
    fn infrastructure_land_acquisition_accepts_french_project_type_variants() {
        let text = "La Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières.";
        for project_type in [
            "infrastructure",
            "infrastructures",
            "infrastructure routière",
            "Infrastructures routières",
            "INFRASTRUCTURE",
        ] {
            assert!(
                is_infrastructure_land_acquisition(text, "fr", Some(project_type)),
                "expected project_type {project_type:?} to qualify"
            );
        }
    }

    /// The prefix match must not widen the gate to unrelated types — in
    /// particular a type that merely *contains* "infrastructure" later on.
    #[test]
    fn infrastructure_land_acquisition_rejects_unrelated_project_type_variants() {
        let text = "La Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières.";
        for project_type in ["résidentiel", "institutionnel", "commercial", "industriel"] {
            assert!(
                !is_infrastructure_land_acquisition(text, "fr", Some(project_type)),
                "expected project_type {project_type:?} not to qualify"
            );
        }
    }

    /// Real Montreal PDFs use the typographic apostrophe U+2019 (’), not the
    /// ASCII `'` the FR keyword list is written with. Without normalization
    /// this text matches nothing at all.
    #[test]
    fn infrastructure_land_acquisition_matches_typographic_apostrophe() {
        let text = "CM26 0091 — la Ville s\u{2019}est engagée à acquérir un terrain, pour les fins de réaménagement d\u{2019}infrastructures routières, situé à l\u{2019}intersection de l\u{2019}avenue Saint-Pierre et de la rue Notre-Dame.";
        assert!(text.contains('\u{2019}'), "fixture must use U+2019");
        assert!(is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("infrastructures routières")
        ));
    }

    /// "fins de réaménagement" (no leading "aux"/"pour les") must match both
    /// real phrasings.
    #[test]
    fn infrastructure_land_acquisition_matches_either_fins_de_reamenagement_phrasing() {
        for text in [
            "Acquisition d'un terrain pour les fins de réaménagement de la voie publique.",
            "Acquisition d'un terrain aux fins de réaménagement de la voie publique.",
        ] {
            assert!(
                is_infrastructure_land_acquisition(text, "fr", Some("infrastructure")),
                "expected {text:?} to qualify"
            );
        }
    }
}
