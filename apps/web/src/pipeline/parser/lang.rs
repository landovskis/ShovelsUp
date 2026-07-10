use whatlang::{detect, Lang};

/// Detects whether `text` is English or French. Returns `None` when
/// `whatlang` can't confidently classify the text (e.g. too short, or
/// neither language) — callers should treat that as "unknown", not default
/// to English, since silently defaulting would corrupt the bilingual
/// (REQ-004/REQ-007) pipeline downstream.
pub fn detect_language(text: &str) -> Option<&'static str> {
    match detect(text)?.lang() {
        Lang::Eng => Some("en"),
        Lang::Fra => Some("fr"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english() {
        assert_eq!(
            detect_language(
                "The council approved the rezoning application for the downtown parcel."
            ),
            Some("en")
        );
    }

    #[test]
    fn detects_french() {
        assert_eq!(
            detect_language(
                "Le conseil municipal a approuvé la demande de modification du zonage."
            ),
            Some("fr")
        );
    }

    #[test]
    fn returns_none_for_too_short_text() {
        assert_eq!(detect_language("ok"), None);
    }
}
