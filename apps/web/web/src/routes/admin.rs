use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;
use shovelsup_pipeline::parser::{ocr::TesseractOcrProvider, orchestrate::parse_and_store};

#[derive(Serialize)]
pub struct ReprocessResponse {
    pub fetch_job_id: Uuid,
    pub status: String,
}

/// POST /admin/fetch_jobs/{id}/reprocess (IMP-REQ-001-07)
///
/// - 404 if the fetch job doesn't exist
/// - 409 if the job is already `pending` or `in_progress` (nothing to reprocess)
/// - 503 on DB failure
/// - 200 with the job reset to `pending` and `attempts` cleared otherwise
pub async fn reprocess_fetch_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ReprocessResponse>, StatusCode> {
    let status: Option<String> =
        sqlx::query_scalar!("SELECT status FROM fetch_jobs WHERE id = $1", id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let status = status.ok_or(StatusCode::NOT_FOUND)?;
    if status == "pending" || status == "in_progress" {
        return Err(StatusCode::CONFLICT);
    }

    sqlx::query!(
        "UPDATE fetch_jobs SET status = 'pending', attempts = 0, last_error = NULL, \
         updated_at = now() WHERE id = $1",
        id
    )
    .execute(&state.db)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Json(ReprocessResponse {
        fetch_job_id: id,
        status: "pending".to_string(),
    }))
}

#[derive(Serialize)]
pub struct ReprocessParsingResponse {
    pub source_document_id: Uuid,
    pub parser_status: String,
    pub chunk_count: i64,
}

/// POST /admin/source_documents/{id}/reprocess (IMP-REQ-002-08)
///
/// Re-parses a source document's stored content regardless of its current
/// `parser_status` — that's the point of a manual reprocess trigger.
///
/// - 404 if the source document doesn't exist
/// - 503 on DB failure
/// - 200 with the resulting `parser_status` (`parsed`/`failed`/`reprocessing`
///   — see IMP-REQ-002-09) and chunk count otherwise
pub async fn reprocess_source_document(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ReprocessParsingResponse>, StatusCode> {
    let exists: Option<Uuid> =
        sqlx::query_scalar!("SELECT id FROM source_documents WHERE id = $1", id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    exists.ok_or(StatusCode::NOT_FOUND)?;

    let chunk_count = parse_and_store(&state.db, id, &TesseractOcrProvider)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let parser_status: String = sqlx::query_scalar!(
        "SELECT parser_status FROM source_documents WHERE id = $1",
        id
    )
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Json(ReprocessParsingResponse {
        source_document_id: id,
        parser_status,
        chunk_count: chunk_count as i64,
    }))
}
