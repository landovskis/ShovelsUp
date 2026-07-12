use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};

/// `REVIEW_QUEUE_ENABLED` (IMP-REQ-009-09, default `false`/unset): when
/// disabled, the review-queue routes 404 as if they don't exist — not 403
/// — so an unauthenticated prober can't distinguish "flag off" from "route
/// never existed", and the nav has no link to it (frontend concern,
/// handled by simply never rendering the link outside this middleware).
pub fn review_queue_enabled() -> bool {
    is_truthy(std::env::var("REVIEW_QUEUE_ENABLED").ok().as_deref())
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
}
