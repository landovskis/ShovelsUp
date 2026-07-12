use std::process::Command;

use super::ParseError;

/// OCR provider is unspecified by the PRD (see Autonomous Execution Notes on
/// REQ-002) — implemented behind this trait so the concrete engine can be
/// swapped without touching call sites. `TesseractOcrProvider` is the
/// conservative default: a local, no-cost engine, avoiding a hosted
/// OCR-provider decision blocking implementation.
pub trait OcrProvider: Send + Sync {
    /// Rasterizes and OCRs `pdf_bytes`, returning one string per page.
    fn ocr_pdf(&self, pdf_bytes: &[u8]) -> Result<Vec<String>, ParseError>;
}

pub struct TesseractOcrProvider;

impl OcrProvider for TesseractOcrProvider {
    fn ocr_pdf(&self, pdf_bytes: &[u8]) -> Result<Vec<String>, ParseError> {
        let dir = std::env::temp_dir().join(format!("shovelsup-ocr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| ParseError::Ocr(format!("failed to create temp dir: {e}")))?;
        let result = run_ocr_pipeline(pdf_bytes, &dir);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}

fn run_ocr_pipeline(pdf_bytes: &[u8], dir: &std::path::Path) -> Result<Vec<String>, ParseError> {
    let pdf_path = dir.join("input.pdf");
    std::fs::write(&pdf_path, pdf_bytes)
        .map_err(|e| ParseError::Ocr(format!("failed to write pdf to temp dir: {e}")))?;

    // pdftoppm/tesseract are poppler-utils/tesseract-ocr binaries — an
    // unavailable or misbehaving binary here is exactly the "OCR worker
    // unavailable" condition TC-REQ-002-4 requires be retryable, not a
    // permanent failure: every branch below returns ParseError::Ocr, which
    // the caller (parse_document orchestration, IMP-REQ-002-09) is
    // responsible for classifying as transient.
    let ppm_prefix = dir.join("page");
    let status = Command::new("pdftoppm")
        .args(["-png", "-r", "300"])
        .arg(&pdf_path)
        .arg(&ppm_prefix)
        .status()
        .map_err(|e| ParseError::Ocr(format!("pdftoppm unavailable: {e}")))?;
    if !status.success() {
        return Err(ParseError::Ocr(format!("pdftoppm exited with {status}")));
    }

    let mut page_files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| ParseError::Ocr(format!("failed to read rendered pages: {e}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("png"))
        .collect();
    page_files.sort();

    let mut pages = Vec::with_capacity(page_files.len());
    for page_file in &page_files {
        let output = Command::new("tesseract")
            .arg(page_file)
            .arg("stdout")
            .args(["-l", "eng+fra"])
            .output()
            .map_err(|e| ParseError::Ocr(format!("tesseract unavailable: {e}")))?;
        if !output.status.success() {
            return Err(ParseError::Ocr(format!(
                "tesseract exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        pages.push(String::from_utf8_lossy(&output.stdout).to_string());
    }

    Ok(pages)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{OcrProvider, ParseError};

    /// Simulates an unavailable/failing OCR worker for TC-REQ-002-4, without
    /// depending on real tesseract/pdftoppm binaries or mutating process-wide
    /// PATH (unsafe under a parallel test runner).
    pub struct FailingOcrProvider;

    impl OcrProvider for FailingOcrProvider {
        fn ocr_pdf(&self, _pdf_bytes: &[u8]) -> Result<Vec<String>, ParseError> {
            Err(ParseError::Ocr(
                "OCR worker unavailable (test double)".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLANK_PAGE_PDF: &[u8] = include_bytes!("../../../tests/fixtures/blank_page.pdf");
    const MALFORMED_PDF: &[u8] = include_bytes!("../../../tests/fixtures/malformed.pdf");

    /// These tests all create/inspect `shovelsup-ocr-*` dirs under the
    /// process temp dir; `cargo test` runs tests in parallel by default, so
    /// without serializing them the cleanup-count test races the others.
    static OCR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Skip real-binary tests in environments without poppler-utils/tesseract
    /// installed, rather than failing the whole suite there.
    fn ocr_toolchain_available() -> bool {
        Command::new("pdftoppm")
            .arg("-v")
            .output()
            .is_ok_and(|o| o.status.success() || o.status.code() == Some(99))
            && Command::new("tesseract")
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
    }

    /// Exercises the real pdftoppm -> tesseract pipeline end to end. A blank
    /// page rasterizes and OCRs successfully with empty/near-empty text —
    /// this must be Ok(_), not an error; an OCR error on a legitimately
    /// blank page would misclassify "nothing to read" as "worker down".
    #[test]
    fn tesseract_provider_ocrs_a_real_rendered_page() {
        let _guard = OCR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !ocr_toolchain_available() {
            eprintln!("skipping: pdftoppm/tesseract not installed");
            return;
        }
        let result = TesseractOcrProvider.ocr_pdf(BLANK_PAGE_PDF);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(result.unwrap().len(), 1, "one page rendered");
    }

    /// pdftoppm itself fails (not just tesseract) on a PDF it can't parse —
    /// this is the "pdftoppm exited non-zero" branch, distinct from a
    /// tesseract failure.
    #[test]
    fn tesseract_provider_reports_pdftoppm_failure_as_ocr_error() {
        let _guard = OCR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !ocr_toolchain_available() {
            eprintln!("skipping: pdftoppm/tesseract not installed");
            return;
        }
        let result = TesseractOcrProvider.ocr_pdf(MALFORMED_PDF);
        assert!(matches!(result, Err(ParseError::Ocr(_))));
    }

    /// The per-call temp directory must not survive the call, on either the
    /// success or failure path — it holds the source PDF and rendered pages.
    #[test]
    fn tesseract_provider_cleans_up_temp_dir_on_success_and_failure() {
        let _guard = OCR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !ocr_toolchain_available() {
            eprintln!("skipping: pdftoppm/tesseract not installed");
            return;
        }
        let before: Vec<_> = glob_shovelsup_ocr_dirs();

        let _ = TesseractOcrProvider.ocr_pdf(BLANK_PAGE_PDF);
        let _ = TesseractOcrProvider.ocr_pdf(MALFORMED_PDF);

        let after: Vec<_> = glob_shovelsup_ocr_dirs();
        assert_eq!(
            before.len(),
            after.len(),
            "no shovelsup-ocr-* temp dirs should remain after either call"
        );
    }

    fn glob_shovelsup_ocr_dirs() -> Vec<std::path::PathBuf> {
        std::fs::read_dir(std::env::temp_dir())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("shovelsup-ocr-"))
            })
            .collect()
    }
}
