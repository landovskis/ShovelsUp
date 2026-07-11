use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize)]
pub struct TimelineEvent {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_mention_id: Uuid,
    pub event_date: DateTime<Utc>,
    pub normalized_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// GET /api/v1/projects/{id}/timeline.
///
/// Events are ordered chronologically; `created_at` provides stable
/// ingestion-order sequencing for equal event timestamps.
pub async fn get_project_timeline(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TimelineEvent>>, StatusCode> {
    let project_exists = sqlx::query_scalar!("SELECT id FROM projects WHERE id = $1", id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    project_exists.ok_or(StatusCode::NOT_FOUND)?;

    let events = sqlx::query!(
        "SELECT id, project_id, project_mention_id, event_date, normalized_status, created_at \
         FROM project_timeline_events WHERE project_id = $1 \
         ORDER BY event_date ASC, created_at ASC",
        id
    )
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .into_iter()
    .map(|event| TimelineEvent {
        id: event.id,
        project_id: event.project_id,
        project_mention_id: event.project_mention_id,
        event_date: event.event_date,
        normalized_status: event.normalized_status,
        created_at: event.created_at,
    })
    .collect();

    Ok(Json(events))
}
