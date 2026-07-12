pub(crate) mod html;
pub(crate) mod lang;
pub mod ocr;
pub mod orchestrate;
pub(crate) mod plaintext;
pub(crate) mod pdf;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unsupported content type: {0}")]
    UnsupportedContentType(String),
    #[error("pdf parse error: {0}")]
    Pdf(String),
    #[error("ocr error: {0}")]
    Ocr(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMethod {
    Text,
    Ocr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedChunk {
    pub content: String,
    pub parse_method: ParseMethod,
}

pub type ParseOutcome = Vec<ParsedChunk>;

/// Dispatch to a format-specific handler by (normalized) MIME type. Rejects
/// unsupported types before any handler runs (TC-REQ-002-3).
pub fn parse_document(
    content_type: &str,
    body: &[u8],
    ocr: &dyn ocr::OcrProvider,
) -> Result<ParseOutcome, ParseError> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();

    match mime.as_str() {
        "text/html" => Ok(html::parse(body)),
        "application/pdf" => pdf::parse(body, ocr),
        "text/plain" => Ok(plaintext::parse(body)),
        _ => Err(ParseError::UnsupportedContentType(mime)),
    }
}

#[cfg(test)]
mod tests {
    use super::ocr::TesseractOcrProvider;
    use super::*;

    /// TC-REQ-002-3: unsupported MIME type rejected before handler dispatch.
    #[test]
    fn parse_document_rejects_unsupported_mime_type() {
        let result = parse_document("application/msword", b"whatever", &TesseractOcrProvider);
        assert!(matches!(result, Err(ParseError::UnsupportedContentType(_))));
    }

    #[test]
    fn parse_document_normalizes_mime_type_with_charset_parameter() {
        let result = parse_document("text/html; charset=utf-8", b"<p>hi</p>", &TesseractOcrProvider);
        assert!(result.is_ok());
    }
}
