# ADR 007 — Redis Fixed-Window Rate Limiting for Public Search

**Status**: Accepted
**Date**: 2026-07-13
**Feature**: Implementation Plan: Public Discovery & Search (IMP-REQ-008-05 / IMP-REQ-001-05 / IMP-REQ-010-07)

## Context

REQ-010 opened `/search` and `/api/v1/projects/search` to fully anonymous, unauthenticated
callers ("no account required"). An unauthenticated, unmetered public endpoint backed by
Postgres full-text search is an easy target for scraping or abuse, so REQ-008 called for
per-client rate limiting on exactly these two routes.

The first implementation keyed the limiter off the client-supplied `X-Forwarded-For`
header. This was a real security bug (IMP-REQ-001-05): `docker-compose.yml`/`Dockerfile`
do not put any reverse proxy (nginx, traefik, etc.) in front of this service — the Axum
server is the only thing terminating the TCP connection — so any anonymous caller could
set an arbitrary `X-Forwarded-For` value to either spoof a fresh "IP" on every request
(bypassing the limit entirely) or frame another IP (spoofing its address to trip that
IP's bucket). The fix was to key the limiter off Axum's `ConnectInfo<SocketAddr>`
extractor instead, which reflects the actual TCP peer address and cannot be forged by
the client; `main.rs` was updated to serve via
`into_make_service_with_connect_info::<SocketAddr>()` so `ConnectInfo` is always
populated in production.

Options considered for the limiting algorithm:

| Option | Description |
|--------|-------------|
| Redis fixed 60s window (`INCR` + `EXPIRE`) | Simple counter per key, resets every 60 seconds |
| Sliding-window log/counter | Smooths bucket-edge bursts, more Redis operations per request |
| Token bucket | Allows burst + steady refill, requires more state per key |
| In-memory (per-process) limiter | No new infra dependency, but doesn't survive restarts and can't be right if ever scaled to multiple instances |

## Decision

Use a **Redis-backed fixed 60-second window** counter, implemented in
`middleware::rate_limit::rate_limit_search`:

- Key: `rate_limit:search:{ip}`, where `{ip}` is `SocketAddr::ip()` taken from
  `ConnectInfo<SocketAddr>` — never from any request header. `ConnectInfo` is extracted as
  `Option` (not required) because it is only populated when the app is actually served via
  `into_make_service_with_connect_info` (as `main.rs` does), not when a `Router` is
  exercised directly via `tower::ServiceExt::oneshot` in most of this crate's integration
  tests. Requests with no `ConnectInfo` fall back to one fixed, non-attacker-controlled
  key rather than 500ing or falling back to a header.
- Each request does `INCR` on the key; the first request to see the key at 1 also sets a
  60-second `EXPIRE`. A crash between `INCR` and `EXPIRE` would leave a key with no expiry
  — accepted as fails-safe (under-limiting, not a livelock).
- Default limit is 60 requests/minute/IP, overridable via `RATE_LIMIT_SEARCH_RPM` (the PRD
  did not set a threshold; a configurable default lets it be tuned post-launch against
  real traffic without a code change).
- Applied only to `/search` and `/api/v1/projects/search`, via a `Router` sub-group with
  its own `.layer(...)`, matching the pre-fix scope exactly.
- On limit exceeded, the middleware returns a bare `429 Too Many Requests` — no CAPTCHA,
  no interactive challenge of any kind. This is a deliberate consequence of REQ-010's "no
  account required" requirement: any interactive friction (a challenge page, a puzzle)
  would itself be a form of the account/interaction barrier REQ-010 exists to avoid, even
  for legitimate anonymous users who simply hit the threshold.
- A Redis outage degrades the endpoint to `503 Service Unavailable` (the `INCR`/`EXPIRE`
  calls' error path), rather than either failing open (unbounded requests) or panicking.

## Rationale

- A fixed window is the simplest mechanism that satisfies "bound requests per IP per
  minute" and needs only two Redis commands per request; sliding-window/token-bucket
  algorithms solve burst-smoothing problems this launch-scale public endpoint doesn't have
  yet.
- Redis was already provisioned in `docker-compose.yml`/`.env` (per ADR 005) but had no
  real caller before this — reusing it avoids a new infrastructure dependency.
- An in-memory limiter would have been simpler still, but ADR 006 already established
  that this app may scale to multiple instances in the future; an in-memory counter would
  silently stop being correct at that point, whereas a Redis-backed counter keeps working
  unchanged.

## Consequences

- **Keying is IP-only, not IP+route.** Both rate-limited routes share configuration but
  each gets its own Redis key (the key embeds the fixed `rate_limit:search:` prefix, not
  a per-route segment) — actually, the key is per-IP across both routes combined, so a
  caller hitting both endpoints shares one budget. This is intentional: both are "the
  public search surface" from an abuse standpoint.
- **No interactive challenge path exists.** If REQ-010's "no account required" constraint
  is ever relaxed, or if abuse patterns emerge that a simple per-IP counter can't stop
  (e.g. distributed scraping across many IPs), this ADR's silent-429 approach should be
  revisited rather than bolted onto in place.
- **Revisit if a reverse proxy is introduced.** If a trusted reverse proxy is added in
  front of this service later, `ConnectInfo` will reflect the proxy's address for every
  request, not the real client's — at that point `X-Forwarded-For` (or a proxy-specific
  header) becomes the correct signal again, but only when validated as coming from a
  known-good upstream hop, not trusted blindly as it was before this fix.
