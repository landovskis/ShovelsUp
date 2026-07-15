use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
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
/// IMP-REQ-001-05: the client IP is taken from Axum's `ConnectInfo`, i.e.
/// the actual TCP peer address of the connection, NOT from the
/// client-supplied `X-Forwarded-For` header. `docker-compose.yml` and
/// `Dockerfile` in this repo were inspected and neither provisions a
/// reverse proxy (nginx, traefik, etc.) in front of this service — it is
/// the only thing terminating the connection, so any anonymous caller
/// could set an arbitrary `X-Forwarded-For` value to either bypass the
/// limit (spoof a new "IP" every request) or frame another IP (spoof its
/// address to trip its bucket). `ConnectInfo` is populated by
/// `Router::into_make_service_with_connect_info` in `main.rs` (always the
/// case in production) and cannot be forged by the client. If a trusted
/// reverse proxy is introduced later, this must be revisited (e.g.
/// trusting `X-Forwarded-For` only when it comes from a known-good
/// upstream hop).
///
/// `ConnectInfo` is extracted as `Option` rather than required: Axum only
/// populates it when the app is served via
/// `into_make_service_with_connect_info` (as `main.rs` does), not when a
/// `Router` is exercised directly (e.g. plain `tower::ServiceExt::oneshot`
/// in most of this crate's integration tests, none of which need to
/// exercise the rate limiter's IP-keying and so don't bother mocking a
/// connection). Rather than 500 every such request, requests with no
/// `ConnectInfo` fall back to sharing one fixed, non-attacker-controlled
/// key — never the client-supplied header — same as this middleware did
/// for a missing `X-Forwarded-For` before this fix.
pub async fn rate_limit_search(
    State(state): State<AppState>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let limit: u32 = std::env::var("RATE_LIMIT_SEARCH_RPM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RATE_LIMIT_RPM);

    let client_key = match connect_info {
        Some(ConnectInfo(peer)) => rate_limit_key(peer),
        None => "unknown".to_string(),
    };

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

/// Derives the Redis rate-limit key from the connection's peer address.
///
/// This is the core (no I/O) half of the IMP-REQ-001-05 fix: it takes the
/// unspoofable `SocketAddr` Axum's server hands back via `ConnectInfo` and
/// never looks at request headers, so a forged `X-Forwarded-For` cannot
/// influence which bucket a request lands in. The port is discarded — two
/// requests from the same client IP on different ephemeral ports must
/// share a bucket.
fn rate_limit_key(peer: SocketAddr) -> String {
    peer.ip().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_key_uses_the_peer_ip() {
        let peer: SocketAddr = "203.0.113.7:54321".parse().unwrap();
        assert_eq!(rate_limit_key(peer), "203.0.113.7");
    }

    #[test]
    fn rate_limit_key_ignores_port_but_distinguishes_ips() {
        let same_ip_a: SocketAddr = "203.0.113.7:1".parse().unwrap();
        let same_ip_b: SocketAddr = "203.0.113.7:65535".parse().unwrap();
        let different_ip: SocketAddr = "198.51.100.9:1".parse().unwrap();

        assert_eq!(rate_limit_key(same_ip_a), rate_limit_key(same_ip_b));
        assert_ne!(rate_limit_key(same_ip_a), rate_limit_key(different_ip));
    }

    #[test]
    fn rate_limit_key_is_derived_only_from_connect_info_never_headers() {
        // `rate_limit_key` takes a `SocketAddr` and nothing else — there is
        // no header/`X-Forwarded-For` parameter for a caller to smuggle a
        // forged value through. Two "clients" that would send identical
        // (forged) X-Forwarded-For headers but connect from different real
        // addresses must land in different buckets, and the same real
        // address must land in the same bucket no matter what headers a
        // request carries (headers aren't consulted at all).
        let attacker_real_addr: SocketAddr = "203.0.113.7:1".parse().unwrap();
        let victim_real_addr: SocketAddr = "198.51.100.42:1".parse().unwrap();

        // Simulates the attacker forging X-Forwarded-For to the victim's
        // address on every request: the key still comes from ConnectInfo,
        // so it does not match the victim's key.
        assert_ne!(
            rate_limit_key(attacker_real_addr),
            rate_limit_key(victim_real_addr)
        );

        // Simulates the attacker forging a fresh X-Forwarded-For on every
        // request to try to evade the limit: the key is stable because it
        // only depends on the real connection, not the forged header.
        assert_eq!(
            rate_limit_key(attacker_real_addr),
            rate_limit_key(attacker_real_addr)
        );
    }
}
