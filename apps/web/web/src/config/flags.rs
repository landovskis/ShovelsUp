use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};

/// `REVIEW_QUEUE_ENABLED` (IMP-REQ-009-09, default `false`/unset): when
/// disabled, the review-queue routes 404 as if they don't exist — not 403
/// — so an unauthenticated prober can't distinguish "flag off" from "route
/// never existed", and the nav has no link to it (frontend concern,
/// handled by simply never rendering the link outside this middleware).
pub fn review_queue_enabled() -> bool {
    is_truthy(std::env::var("REVIEW_QUEUE_ENABLED").ok().as_deref())
}

/// `DATA_PIPELINE_INGESTION_ENABLED` (IMP-REQ-001-12, default `false`/unset):
/// gates the fetch-job worker's interval loop in `main.rs`. Read live on
/// every tick (not cached at startup) so ops can flip it without a restart
/// (docs/runbooks/data_pipeline_ingestion.md). Do not enable in an
/// environment where the seeded municipality domains
/// (migrations/002_seed_municipalities.sql) haven't had legal/public-source
/// sign-off.
pub fn data_pipeline_ingestion_enabled() -> bool {
    is_truthy(std::env::var("DATA_PIPELINE_INGESTION_ENABLED").ok().as_deref())
}

fn is_truthy(value: Option<&str>) -> bool {
    matches!(value, Some("true") | Some("1"))
}

pub async fn require_review_queue_enabled(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !review_queue_enabled() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_when_unset_or_falsy() {
        assert!(!is_truthy(None));
        assert!(!is_truthy(Some("false")));
        assert!(!is_truthy(Some("")));
    }

    #[test]
    fn enabled_for_true_or_one() {
        assert!(is_truthy(Some("true")));
        assert!(is_truthy(Some("1")));
    }

    #[test]
    fn data_pipeline_ingestion_disabled_when_unset_or_falsy() {
        assert!(!is_truthy(None));
        // data_pipeline_ingestion_enabled() itself reads the real env var,
        // so it's exercised indirectly via is_truthy here (same helper,
        // same contract as review_queue_enabled) — a dedicated env-var
        // integration test would be flaky under parallel test execution
        // (shared process env), matching why review_queue_enabled has no
        // such test either.
    }
}
