//! Admin routes for the human-review queue (IMP-REQ-009-04).
//!
//! Path deviation (flagged, matching the same convention already used for
//! `routes/admin.rs`, `routes/search.rs`, `routes/projects.rs`): the plan's
//! Target Files / Modules column names `apps/web/src/routes/admin/review_queue.rs`,
//! but `routes::admin` is an existing flat module (`admin.rs`), not a
//! directory — this file sits alongside it as a flat sibling instead of
//! converting `admin.rs` into a directory for this one addition.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use shovelsup_domain::review_queue::{confirm_candidate, reject_candidate, ReviewQueueError};
use crate::routes::detect_lang;
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct ReviewCandidateSummary {
    pub id: Uuid,
    pub candidate_type: String,
    pub status: String,
    pub version: i32,
    pub due_at: chrono::DateTime<chrono::Utc>,
    pub overdue: bool,
    pub details: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Defaults to `"open"` — the queue's default "Open" tab.
    pub status: Option<String>,
}

async fn fetch_candidates(
    pool: &sqlx::PgPool,
    status: &str,
) -> Result<Vec<ReviewCandidateSummary>, sqlx::Error> {
    let rows = sqlx::query!(
        "SELECT id, candidate_type, status, version, due_at, details, (due_at < now()) AS \"overdue!\" \
         FROM review_candidates WHERE status = $1 ORDER BY due_at ASC",
        status
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| ReviewCandidateSummary {
        id: row.id,
        candidate_type: row.candidate_type,
        status: row.status,
        version: row.version,
        due_at: row.due_at,
        overdue: row.overdue,
        details: row.details,
    })
    .collect();

    Ok(rows)
}

/// GET /admin/review_candidates?status=open|confirmed|rejected
/// (TC-REQ-009-4: a multi-match candidate created by REQ-005's resolver
/// appears here under the Open tab, since it's created with status='open').
pub async fn list_review_candidates(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ReviewCandidateSummary>>, StatusCode> {
    let status = params.status.unwrap_or_else(|| "open".to_string());
    let rows = fetch_candidates(&state.db, &status)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(rows))
}

/// GET /admin/review_candidates/{id}
pub async fn get_review_candidate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ReviewCandidateSummary>, StatusCode> {
    let row = sqlx::query!(
        "SELECT id, candidate_type, status, version, due_at, details, (due_at < now()) AS \"overdue!\" \
         FROM review_candidates WHERE id = $1",
        id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(ReviewCandidateSummary {
        id: row.id,
        candidate_type: row.candidate_type,
        status: row.status,
        version: row.version,
        due_at: row.due_at,
        overdue: row.overdue,
        details: row.details,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub version: i32,
    pub project_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct RejectBody {
    pub version: i32,
}

fn map_review_queue_error(err: ReviewQueueError) -> StatusCode {
    match err {
        ReviewQueueError::NotFound(_) => StatusCode::NOT_FOUND,
        ReviewQueueError::NotOpen(_) => StatusCode::CONFLICT,
        ReviewQueueError::VersionConflict => StatusCode::CONFLICT,
        ReviewQueueError::Db(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// POST /admin/review_candidates/{id}/confirm
/// (TC-REQ-009-1, TC-REQ-009-3, TC-REQ-009-5). `actor` is `ADMIN_USER` —
/// this app has a single shared admin account (HTTP Basic), not
/// per-operator sessions, so that's the most specific identity available.
pub async fn confirm_review_candidate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ConfirmBody>,
) -> Result<StatusCode, StatusCode> {
    let actor = std::env::var("ADMIN_USER").unwrap_or_else(|_| "admin".to_string());
    confirm_candidate(&state.db, id, body.version, body.project_id, &actor)
        .await
        .map_err(map_review_queue_error)?;
    Ok(StatusCode::OK)
}

/// POST /admin/review_candidates/{id}/reject
pub async fn reject_review_candidate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RejectBody>,
) -> Result<StatusCode, StatusCode> {
    let actor = std::env::var("ADMIN_USER").unwrap_or_else(|_| "admin".to_string());
    reject_candidate(&state.db, id, body.version, &actor)
        .await
        .map_err(map_review_queue_error)?;
    Ok(StatusCode::OK)
}

struct QueueLabels {
    page_title: &'static str,
    heading: &'static str,
    tab_open: &'static str,
    tab_confirmed: &'static str,
    tab_rejected: &'static str,
    empty_message: &'static str,
    confirm_label: &'static str,
    reject_label: &'static str,
    overdue_label: &'static str,
    stale_conflict_message: &'static str,
    nav_permits: &'static str,
    nav_council: &'static str,
}

fn queue_labels(lang: &str) -> QueueLabels {
    match lang {
        "fr" => QueueLabels {
            page_title: "File de révision",
            heading: "File de révision humaine",
            tab_open: "Ouvertes",
            tab_confirmed: "Confirmées",
            tab_rejected: "Rejetées",
            empty_message: "Aucun candidat dans cet onglet.",
            confirm_label: "Confirmer",
            reject_label: "Rejeter",
            overdue_label: "En retard",
            stale_conflict_message: "Ce candidat a changé depuis son chargement. Actualisez et réessayez.",
            nav_permits: "Permis",
            nav_council: "Conseil",
        },
        _ => QueueLabels {
            page_title: "Review queue",
            heading: "Human review queue",
            tab_open: "Open",
            tab_confirmed: "Confirmed",
            tab_rejected: "Rejected",
            empty_message: "No candidates in this tab.",
            confirm_label: "Confirm",
            reject_label: "Reject",
            overdue_label: "Overdue",
            stale_conflict_message: "This candidate has changed since it was loaded. Refresh and try again.",
            nav_permits: "Permits",
            nav_council: "Council",
        },
    }
}

#[derive(Debug, Deserialize)]
pub struct QueuePageParams {
    pub status: Option<String>,
}

/// GET /admin/review_queue — server-rendered review-queue page
/// (IMP-REQ-009-06), tabs/states/EN-FR. Confirm/Reject buttons are wired
/// client-side (IMP-REQ-009-07, `static/js/review_queue.js`) against the
/// JSON routes above, since a stale-version 409 needs to show an inline
/// banner without a full page reload.
pub async fn get_review_queue_page(
    State(state): State<AppState>,
    Query(params): Query<QueuePageParams>,
    headers: HeaderMap,
) -> Result<Html<String>, StatusCode> {
    let lang = detect_lang(&headers);
    let labels = queue_labels(lang);
    let active_tab = params.status.unwrap_or_else(|| "open".to_string());

    let tmpl = state
        .env
        .get_template("admin/review_queue.html")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let candidates = fetch_candidates(&state.db, &active_tab)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let html = tmpl
        .render(context! {
            lang => lang,
            nav_permits => labels.nav_permits,
            nav_council => labels.nav_council,
            page_title => labels.page_title,
            heading => labels.heading,
            tab_open => labels.tab_open,
            tab_confirmed => labels.tab_confirmed,
            tab_rejected => labels.tab_rejected,
            empty_message => labels.empty_message,
            confirm_label => labels.confirm_label,
            reject_label => labels.reject_label,
            overdue_label => labels.overdue_label,
            stale_conflict_message => labels.stale_conflict_message,
            active_tab => active_tab,
            candidates => candidates,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(html))
}
