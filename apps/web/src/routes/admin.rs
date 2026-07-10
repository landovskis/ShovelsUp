use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;

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
