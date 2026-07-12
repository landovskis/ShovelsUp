use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;

/// HTTP Basic Auth against `ADMIN_USER` / `ADMIN_PASSWORD_HASH` (bcrypt).
/// Rejects with 403 before any handler logic runs when credentials are
/// missing, malformed, or don't match — reused across every admin-only route
/// (REQ-001 reprocess, REQ-002 reprocess, REQ-009 review queue).
pub async fn require_admin(req: Request, next: Next) -> Result<Response, StatusCode> {
    let admin_user = std::env::var("ADMIN_USER").map_err(|_| StatusCode::FORBIDDEN)?;
    let admin_password_hash =
        std::env::var("ADMIN_PASSWORD_HASH").map_err(|_| StatusCode::FORBIDDEN)?;

    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;

    let encoded = auth_header
        .strip_prefix("Basic ")
        .ok_or(StatusCode::FORBIDDEN)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let credentials = String::from_utf8(decoded).map_err(|_| StatusCode::FORBIDDEN)?;
    let (user, password) = credentials.split_once(':').ok_or(StatusCode::FORBIDDEN)?;

    if user != admin_user {
        return Err(StatusCode::FORBIDDEN);
    }
    match bcrypt::verify(password, &admin_password_hash) {
        Ok(true) => Ok(next.run(req).await),
        _ => Err(StatusCode::FORBIDDEN),
    }
}
