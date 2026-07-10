use std::io::Write;
use std::process::{Command, Stdio};

use super::ocr::OcrProvider;
use super::{ParseError, ParseMethod, ParsedChunk};

/// Below this many total extracted characters, a PDF is treated as scanned
/// (image-only) rather than native-text and routed to OCR.
const MIN_CHARS_TO_SKIP_OCR: usize = 20;

/// Parses a PDF via `pdftotext` (IMP-REQ-002-04). If the extracted text is
/// below the scanned-document threshold, falls back to `ocr` (IMP-REQ-002-05)
/// and tags the resulting chunks `ParseMethod::Ocr`.
pub fn parse(body: &[u8], ocr: &dyn OcrProvider) -> Result<Vec<ParsedChunk>, ParseError> {
    let text = run_pdftotext(body)?;
    let pages: Vec<&str> = text.split('\u{c}').collect();

    if should_ocr(&pages) {
        let ocr_pages = ocr.ocr_pdf(body)?;
        return Ok(pages_to_chunks(&ocr_pages, ParseMethod::Ocr));
    }

    let text_pages: Vec<String> = pages.iter().map(|s| s.to_string()).collect();
    Ok(pages_to_chunks(&text_pages, ParseMethod::Text))
}

fn pages_to_chunks(pages: &[String], method: ParseMethod) -> Vec<ParsedChunk> {
    pages
        .iter()
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .map(|content| ParsedChunk {
            content,
            parse_method: method,
        })
        .collect()
}

fn should_ocr(pages: &[&str]) -> bool {
    let total_chars: usize = pages.iter().map(|p| p.trim().len()).sum();
    total_chars < MIN_CHARS_TO_SKIP_OCR
}

fn run_pdftotext(body: &[u8]) -> Result<String, ParseError> {
    let mut child = Command::new("pdftotext")
        .args(["-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ParseError::Pdf(format!("failed to spawn pdftotext: {e}")))?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(body)
        .map_err(|e| ParseError::Pdf(format!("failed to write to pdftotext stdin: {e}")))?;

    let output = child
        .wait_with_output()
        .map_err(|e| ParseError::Pdf(format!("failed to read pdftotext output: {e}")))?;

    if !output.status.success() {
        return Err(ParseError::Pdf(format!(
            "pdftotext exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::super::ocr::test_support::FailingOcrProvider;
    use super::*;

    struct NeverCalledOcrProvider;
    impl OcrProvider for NeverCalledOcrProvider {
        fn ocr_pdf(&self, _pdf_bytes: &[u8]) -> Result<Vec<String>, ParseError> {
            panic!("OCR should not be invoked for a native-text PDF");
        }
    }

    const MINIMAL_TEXT_PDF: &[u8] = include_bytes!("../../../tests/fixtures/minimal_text.pdf");
    const BLANK_PAGE_PDF: &[u8] = include_bytes!("../../../tests/fixtures/blank_page.pdf");

    #[test]
    fn should_ocr_true_for_near_empty_text() {
        assert!(should_ocr(&["", "   "]));
    }

    #[test]
    fn should_ocr_false_for_substantial_text() {
        assert!(!should_ocr(&["This page has plenty of real extracted text content."]));
    }

    /// TC-REQ-002-1 (PDF half): native-text PDF parses into ordered chunks
    /// without invoking OCR.
    #[test]
    fn parse_native_text_pdf_does_not_invoke_ocr() {
        let chunks = parse(MINIMAL_TEXT_PDF, &NeverCalledOcrProvider).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Hello World"));
        assert_eq!(chunks[0].parse_method, ParseMethod::Text);
    }

    /// TC-REQ-002-4: OCR worker unavailability is retryable, not a permanent
    /// failure — a scanned (near-empty-text) PDF routes to OCR, and an OCR
    /// failure surfaces as ParseError::Ocr rather than corrupting/dropping
    /// data silently.
    #[test]
    fn parse_scanned_pdf_with_unavailable_ocr_returns_ocr_error() {
        let result = parse(BLANK_PAGE_PDF, &FailingOcrProvider);
        assert!(matches!(result, Err(ParseError::Ocr(_))));
    }
}
