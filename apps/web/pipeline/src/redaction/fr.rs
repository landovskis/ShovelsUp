use super::redact_named_individuals;

/// French honorifics that commonly precede a named individual in Quebec
/// municipal proceedings text.
const FR_HONORIFICS: &[&str] = &["M.", "Mme", "Mlle", "Dr", "Dre", "Me", "Monsieur", "Madame"];

const REDACTION_MARKER: &str = "[nom retiré]";

/// Redacts named individuals from a French excerpt (IMP-REQ-007-04).
pub fn redact(text: &str) -> String {
    redact_named_individuals(text, FR_HONORIFICS, REDACTION_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_name_following_monsieur_abbreviation() {
        let text = "M. Jean Tremblay a présenté la demande au conseil.";
        assert_eq!(
            redact(text),
            "M. [nom retiré] a présenté la demande au conseil."
        );
    }

    #[test]
    fn redacts_name_following_madame_abbreviation() {
        let text = "Mme Sophie Bergeron a demandé un permis de construction.";
        assert_eq!(
            redact(text),
            "Mme [nom retiré] a demandé un permis de construction."
        );
    }

    #[test]
    fn redacts_single_token_name() {
        let text = "Dr Gagnon a comparu devant le comité.";
        assert_eq!(redact(text), "Dr [nom retiré] a comparu devant le comité.");
    }

    #[test]
    fn leaves_excerpt_without_honorific_untouched() {
        let text =
            "Construction d'un nouveau bâtiment résidentiel au 123, rue Principale. Approuvé.";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn does_not_redact_unrelated_capitalized_project_or_street_names() {
        let text = "Le projet Riverside Commons au 123, rue Principale a été approuvé.";
        assert_eq!(redact(text), text);
    }
}
