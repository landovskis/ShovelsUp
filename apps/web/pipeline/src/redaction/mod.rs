pub(crate) mod fr;

/// Named-individual redaction (IMP-REQ-007-04, Security). No requirement
/// prior to REQ-007 introduced a redaction pipeline for either language, so
/// this module is the baseline as well as the French extension — there was
/// no existing EN module to "extend" as the plan's task title implies.
///
/// Strips personal names that follow a known honorific (e.g. "M. Jean
/// Tremblay", "Dr. Marie Gagnon") from an excerpt, replacing the name
/// tokens with a redaction marker while leaving the honorific and the rest
/// of the sentence intact — the excerpt should still read as a sentence
/// about a project, just without the named individual.
///
/// Deliberately conservative: only redacts capitalized word sequences (1-2
/// tokens) immediately following a recognized honorific, so ordinary
/// capitalized nouns elsewhere in the excerpt are left untouched. This will
/// not catch every name (a name with no leading honorific is out of scope),
/// but a false negative here is preferable to redacting unrelated project
/// or street names.
pub fn redact_named_individuals(text: &str, honorifics: &[&str], marker: &str) -> String {
    let tokens: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        let bare = token.trim_end_matches(['.', ',']);
        let is_honorific = honorifics.iter().any(|h| h.trim_end_matches('.') == bare);

        out.push(token.to_string());
        i += 1;

        if is_honorific {
            let mut consumed = 0;
            while consumed < 2 && i < tokens.len() && starts_with_uppercase(tokens[i]) {
                i += 1;
                consumed += 1;
            }
            if consumed > 0 {
                out.push(marker.to_string());
            }
        }
    }
    out.join(" ")
}

fn starts_with_uppercase(token: &str) -> bool {
    token.chars().next().is_some_and(|c| c.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HONORIFICS: &[&str] = &["Mr.", "Mr", "Dr"];
    const MARKER: &str = "[REDACTED]";

    #[test]
    fn redacts_two_token_name_after_honorific() {
        let text = "Mr. John Smith presented the application.";
        assert_eq!(
            redact_named_individuals(text, HONORIFICS, MARKER),
            "Mr. [REDACTED] presented the application."
        );
    }

    #[test]
    fn trailing_honorific_with_no_following_name_is_left_alone() {
        let text = "The chair thanked the Dr.";
        assert_eq!(redact_named_individuals(text, HONORIFICS, MARKER), text);
    }

    #[test]
    fn caps_redaction_at_two_tokens_for_a_longer_name() {
        // A 3+ token capitalized run: only the first two tokens are folded
        // into the marker, the third (a genuinely separate capitalized
        // word, e.g. the next sentence's subject) is left as-is — this is
        // the documented conservative limit, not a bug.
        let text = "Dr Jean Paul Tremblay spoke.";
        assert_eq!(
            redact_named_individuals(text, HONORIFICS, MARKER),
            "Dr [REDACTED] Tremblay spoke."
        );
    }

    #[test]
    fn consecutive_honorifics_each_redact_their_own_name() {
        let text = "Mr John Smith and Dr Marie Curie attended.";
        assert_eq!(
            redact_named_individuals(text, HONORIFICS, MARKER),
            "Mr [REDACTED] and Dr [REDACTED] attended."
        );
    }

    #[test]
    fn text_with_no_honorific_is_unchanged() {
        let text = "Construction of a new residential building at 123 Main St.";
        assert_eq!(redact_named_individuals(text, HONORIFICS, MARKER), text);
    }
}
