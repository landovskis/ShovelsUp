use sqlx::PgPool;
use uuid::Uuid;

use super::{lang, ocr::OcrProvider, parse_document, ParseError, ParseMethod};

/// Parses `source_document_id`'s stored content, replaces its
/// `document_chunks`, and updates `parser_status` to reflect the outcome:
///
/// - Success: `parsed`, chunks (re)written.
/// - `ParseError::UnsupportedContentType`: `failed` — permanent, retrying
///   with the same content type can never succeed.
/// - `ParseError::Pdf` / `ParseError::Ocr`: `reprocessing` — transient tool
///   failures (IMP-REQ-002-09; TC-REQ-002-4's "retryable, not permanent").
///
/// Returns the number of chunks written (0 on any parse failure). Only a
/// database error surfaces as `Err` — a parse failure is a handled, recorded
/// outcome, not an error from this function's point of view.
pub async fn parse_and_store(
    pool: &PgPool,
    source_document_id: Uuid,
    ocr: &dyn OcrProvider,
) -> Result<usize, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT content, content_type FROM source_documents WHERE id = $1",
        source_document_id
    )
    .fetch_one(pool)
    .await?;

    let content_type = row.content_type.unwrap_or_default();

    match parse_document(&content_type, &row.content, ocr) {
        Ok(chunks) => {
            let mut tx = pool.begin().await?;
            sqlx::query!(
                "DELETE FROM document_chunks WHERE source_document_id = $1",
                source_document_id
            )
            .execute(&mut *tx)
            .await?;

            for (index, chunk) in chunks.iter().enumerate() {
                let language = lang::detect_language(&chunk.content);
                let parse_method = match chunk.parse_method {
                    ParseMethod::Text => "text",
                    ParseMethod::Ocr => "ocr",
                };
                sqlx::query!(
                    "INSERT INTO document_chunks \
                     (source_document_id, chunk_index, content, language, parse_method) \
                     VALUES ($1, $2, $3, $4, $5)",
                    source_document_id,
                    index as i32,
                    chunk.content,
                    language,
                    parse_method
                )
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query!(
                "UPDATE source_documents SET parser_status = 'parsed' WHERE id = $1",
                source_document_id
            )
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(chunks.len())
        }
        Err(err) => {
            let status = match err {
                ParseError::UnsupportedContentType(_) => "failed",
                ParseError::Pdf(_) | ParseError::Ocr(_) => "reprocessing",
            };
            sqlx::query!(
                "UPDATE source_documents SET parser_status = $1 WHERE id = $2",
                status,
                source_document_id
            )
            .execute(pool)
            .await?;
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::ocr::test_support::FailingOcrProvider;
    use super::super::ocr::TesseractOcrProvider;
    use super::*;

    async fn seed_source_document(pool: &PgPool, content: &[u8], content_type: &str) -> Uuid {
        let municipality_id = sqlx::query_scalar!(
            "INSERT INTO municipalities (name, slug, domain_allowlist) \
             VALUES ('Test City', 'test-city', ARRAY['test-city.example']) RETURNING id"
        )
        .fetch_one(pool)
        .await
        .unwrap();

        sqlx::query_scalar!(
            "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
             VALUES ($1, 'https://test-city.example/doc', 'chk', $2, $3) RETURNING id",
            municipality_id,
            content,
            content_type,
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn parse_and_store_writes_chunks_and_marks_parsed(pool: PgPool) {
        let doc_id = seed_source_document(&pool, b"<p>Council approved the item.</p>", "text/html").await;

        let count = parse_and_store(&pool, doc_id, &TesseractOcrProvider).await.unwrap();
        assert_eq!(count, 1);

        let status: String =
            sqlx::query_scalar!("SELECT parser_status FROM source_documents WHERE id = $1", doc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "parsed");

        let chunk_count: i64 = sqlx::query_scalar!(
            "SELECT count(*) FROM document_chunks WHERE source_document_id = $1",
            doc_id
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(chunk_count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn parse_and_store_marks_failed_for_unsupported_content_type(pool: PgPool) {
        let doc_id = seed_source_document(&pool, b"binary junk", "application/msword").await;

        let count = parse_and_store(&pool, doc_id, &TesseractOcrProvider).await.unwrap();
        assert_eq!(count, 0);

        let status: String =
            sqlx::query_scalar!("SELECT parser_status FROM source_documents WHERE id = $1", doc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
    }

    /// TC-REQ-002-4: OCR worker unavailability marks `reprocessing`
    /// (retryable), not `failed` (permanent).
    #[sqlx::test(migrations = "./migrations")]
    async fn parse_and_store_marks_reprocessing_on_transient_ocr_failure(pool: PgPool) {
        let blank_pdf = include_bytes!("../../../../tests/fixtures/blank_page.pdf");
        let doc_id = seed_source_document(&pool, blank_pdf, "application/pdf").await;

        let count = parse_and_store(&pool, doc_id, &FailingOcrProvider).await.unwrap();
        assert_eq!(count, 0);

        let status: String =
            sqlx::query_scalar!("SELECT parser_status FROM source_documents WHERE id = $1", doc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "reprocessing");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn parse_and_store_reprocessing_replaces_prior_chunks(pool: PgPool) {
        let doc_id = seed_source_document(&pool, b"<p>First version.</p>", "text/html").await;
        parse_and_store(&pool, doc_id, &TesseractOcrProvider).await.unwrap();

        sqlx::query!(
            "UPDATE source_documents SET content = $1 WHERE id = $2",
            b"<p>Second version.</p><p>With more content.</p>".as_slice(),
            doc_id
        )
        .execute(&pool)
        .await
        .unwrap();

        let count = parse_and_store(&pool, doc_id, &TesseractOcrProvider).await.unwrap();
        assert_eq!(count, 2);

        let chunk_count: i64 = sqlx::query_scalar!(
            "SELECT count(*) FROM document_chunks WHERE source_document_id = $1",
            doc_id
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(chunk_count, 2, "stale chunks from the first parse must be replaced");
    }
}
