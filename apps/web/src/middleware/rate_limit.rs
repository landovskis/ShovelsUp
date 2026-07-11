use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use redis::AsyncCommands;

use crate::AppState;

const DEFAULT_RATE_LIMIT_RPM: u32 = 60;

/// Per-IP rate limiting for the public search endpoints (IMP-REQ-008-05),
/// default 60 requests/minute/IP via `RATE_LIMIT_SEARCH_RPM` (Autonomous
/// Execution Notes: threshold not set by the PRD — configurable via env var
/// so it can be tuned post-launch against real traffic without a code
/// change). A fixed 60-second window, keyed per IP via Redis `INCR` +
/// `EXPIRE` — simple and sufficient for a launch-scale public endpoint;
/// not a sliding-window/token-bucket implementation.
///
/// Client IP is read from the `X-Forwarded-For` header (first entry) with
/// no `ConnectInfo` fallback — this app has no reverse-proxy config
/// documented yet, so a request with no such header is bucketed under a
/// shared `"unknown"` key rather than rejected or trusted at face value.
/// Revisit once the real deployment topology (behind a proxy or not) is
/// decided.
pub async fn rate_limit_search(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let limit: u32 = std::env::var("RATE_LIMIT_SEARCH_RPM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RATE_LIMIT_RPM);

    let client_key = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim())
        .unwrap_or("unknown");

    let redis_key = format!("rate_limit:search:{client_key}");
    let mut redis = state.redis.clone();

    let count: u32 = redis
        .incr(&redis_key, 1u32)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if count == 1 {
        // First request in this window: start the 60s TTL. A crash between
        // INCR and EXPIRE would leave a key with no expiry — acceptable for
        // a rate limiter (fails safe toward under-limiting, not a livelock).
        let _: () = redis
            .expire(&redis_key, 60)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    }

    if count > limit {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(next.run(req).await)
}
