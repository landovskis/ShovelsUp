use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use chrono::{DateTime, Utc};
use minijinja::context;
use serde::Serialize;
use uuid::Uuid;

use crate::{routes::detect_lang, AppState};

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

    let events = fetch_timeline_events(&state.db, id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Json(events))
}

async fn fetch_timeline_events(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Vec<TimelineEvent>, sqlx::Error> {
    let events = sqlx::query!(
        "SELECT id, project_id, project_mention_id, event_date, normalized_status, created_at \
         FROM project_timeline_events WHERE project_id = $1 \
         ORDER BY event_date ASC, created_at ASC",
        project_id
    )
    .fetch_all(db)
    .await?
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

    Ok(events)
}

struct TimelineLabels {
    page_title: &'static str,
    timeline_title: &'static str,
    timeline_error_message: &'static str,
    retry_label: &'static str,
    timeline_empty_message: &'static str,
    status_update_fallback: &'static str,
    nav_permits: &'static str,
    nav_council: &'static str,
}

fn timeline_labels(lang: &str) -> TimelineLabels {
    match lang {
        "fr" => TimelineLabels {
            page_title: "Détails du projet",
            timeline_title: "Historique du projet",
            timeline_error_message: "Nous n’avons pas pu charger l’historique de ce projet.",
            retry_label: "Réessayer",
            timeline_empty_message: "Aucun événement n’a encore été enregistré pour ce projet.",
            status_update_fallback: "Mise à jour enregistrée",
            nav_permits: "Permis",
            nav_council: "Conseil",
        },
        _ => TimelineLabels {
            page_title: "Project details",
            timeline_title: "Project timeline",
            timeline_error_message: "We couldn’t load this project’s timeline.",
            retry_label: "Retry timeline",
            timeline_empty_message: "No timeline events have been recorded for this project yet.",
            status_update_fallback: "Update recorded",
            nav_permits: "Permits",
            nav_council: "Council",
        },
    }
}

/// GET /projects/{id}.
///
/// Server-rendered project-detail page: renders the timeline inline (no
/// separate loading state, since the initial page load already has the
/// data) or the error state if the DB is unavailable, per TC-REQ-006-6.
pub async fn get_project_detail_page(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<(StatusCode, Html<String>), StatusCode> {
    let lang = detect_lang(&headers);
    let labels = timeline_labels(lang);

    let tmpl = state
        .env
        .get_template("project_detail.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let render_error_page = |labels: &TimelineLabels| -> Result<(StatusCode, Html<String>), StatusCode> {
        let html = tmpl
            .render(context! {
                lang => lang,
                nav_permits => labels.nav_permits,
                nav_council => labels.nav_council,
                page_title => labels.page_title,
                timeline_title => labels.timeline_title,
                timeline_error => true,
                timeline_error_message => labels.timeline_error_message,
                retry_label => labels.retry_label,
                timeline_retry_url => format!("/projects/{id}"),
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok((StatusCode::SERVICE_UNAVAILABLE, Html(html)))
    };

    let project_exists = match sqlx::query_scalar!("SELECT id FROM projects WHERE id = $1", id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(exists) => exists,
        Err(_) => return render_error_page(&labels),
    };
    project_exists.ok_or(StatusCode::NOT_FOUND)?;

    let events = match fetch_timeline_events(&state.db, id).await {
        Ok(events) => events,
        Err(_) => return render_error_page(&labels),
    };

    let html = tmpl
        .render(context! {
            lang => lang,
            nav_permits => labels.nav_permits,
            nav_council => labels.nav_council,
            page_title => labels.page_title,
            timeline_title => labels.timeline_title,
            timeline_empty_message => labels.timeline_empty_message,
            status_update_fallback => labels.status_update_fallback,
            timeline_events => events,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::OK, Html(html)))
}
