#[derive(Debug, Clone, Copy)]
pub struct VocabularyEntry<'a> {
    pub phrase: &'a str,
    pub normalized_status: &'a str,
}

pub fn normalize_status<'a>(
    raw_text: &str,
    vocabulary: impl IntoIterator<Item = VocabularyEntry<'a>>,
) -> Option<&'a str> {
    let cleaned = raw_text.to_lowercase();

    vocabulary
        .into_iter()
        .filter(|entry| cleaned.contains(entry.phrase))
        .max_by_key(|entry| entry.phrase.len())
        .map(|entry| entry.normalized_status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_matching_phrase_wins() {
        let vocabulary = [
            VocabularyEntry {
                phrase: "approved",
                normalized_status: "generic",
            },
            VocabularyEntry {
                phrase: "approved unanimously",
                normalized_status: "approved",
            },
        ];

        assert_eq!(
            normalize_status("Approved unanimously.", vocabulary),
            Some("approved")
        );
    }

    #[test]
    fn unmatched_text_has_no_default() {
        let vocabulary = [VocabularyEntry {
            phrase: "approved",
            normalized_status: "approved",
        }];
        assert_eq!(normalize_status("Tabled for study", vocabulary), None);
    }
}
