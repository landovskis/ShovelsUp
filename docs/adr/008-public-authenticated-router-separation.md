# ADR 008 — Structurally Separate Public and Authenticated Routers

**Status**: Accepted
**Date**: 2026-07-13
**Feature**: Implementation Plan: Public Discovery & Search (IMP-REQ-010-02)

## Context

REQ-010 requires a public search/browse surface that is reachable with no account or
credential of any kind, alongside the existing admin area (fetch-job/source-document
reprocessing, the review queue) which must stay behind `middleware::admin_auth::
require_admin`.

Before this change, admin routes and public routes were merged inline into one `Router`,
with `.layer(require_admin)` applied directly to the admin sub-router before it was
merged into the whole. This had a latent bug: an unmatched path anywhere in the app (a
typo'd URL, any route never registered) could resolve to whichever sub-router's own
implicit fallback the merge happened to promote to the combined router's catch-all,
depending on merge order. When that promoted fallback belonged to the admin sub-router,
an anonymous visitor hitting a completely unrelated, never-registered path received a
`403` instead of a normal `404` — leaking the fact that an auth-gated area exists at a
URL they never asked about, which is itself a minor information disclosure and directly
undermines REQ-010's "no account required, no friction" public surface.

Options considered:

| Option | Description |
|--------|-------------|
| Keep one merged router, audit `.layer()`/fallback ordering by convention | Relies on developers noticing merge-order-sensitive behavior on every future route addition |
| Two structurally separate router-building functions, explicit top-level fallback | Each function's own auth posture is fixed by construction; a single always-applies fallback removes any dependence on merge order |
| Per-route middleware (attach `require_admin` to each admin route individually, no grouping) | Same risk as the merged-router case, just spread across more call sites |

## Decision

Split route construction into two functions in `src/lib.rs`:

- **`public_router(state: AppState) -> Router<AppState>`** — every route registered here
  must remain reachable without any credential. This function must never have
  `require_admin` (or any other auth-challenge-issuing layer) applied to it, anywhere, by
  construction. The one exception is `rate_limit_search` (ADR 007), layered on the
  `/search`/`/api/v1/projects/search` pair only — it degrades to a plain `429` with no
  interactive challenge, so it doesn't compromise the "no account required" guarantee.
- **`authenticated_router() -> Router<AppState>`** — every route here has
  `axum_middleware::from_fn(middleware::admin_auth::require_admin)` applied at
  construction time, before it is ever merged into `app()`. It is structurally impossible
  to reach any route in this function without passing that layer first.
- **`app(state: AppState) -> Router`** — merges `public_router(state)` and
  `authenticated_router()`, then sets an explicit top-level `.fallback(not_found)` on the
  fully merged, outermost `Router`, i.e. after both sub-routers (each already wrapped in
  its own `.layer(...)`, if any) have been merged in. Because the fallback is applied last
  on the outer router, it can never end up wrapped by `authenticated_router()`'s
  `require_admin` layer — an unmatched path always gets a plain `404`, never a `403`,
  regardless of merge order.

`tests/no_account_required.rs`'s `tc_010_08_public_router_has_no_auth_layer` builds
`public_router` directly (merged with nothing else) and asserts every one of its routes
never returns 401/403 even when sent a forged `Authorization` header that `require_admin`
would reject — the strongest available proof, short of private-field introspection, that
`public_router`'s own construction never pulls in that layer.

## Rationale

- Making each function responsible for its own auth posture by construction means a
  future route added to the wrong function is caught by that function's own doc comment
  and test (`tc_010_08_...`), rather than depending on someone correctly reasoning about
  merge order every time a route is added.
- An explicit top-level fallback is a one-line fix that eliminates an entire class of
  merge-order bugs, at negligible cost.
- Per-route middleware attachment was rejected because it doesn't fix the actual bug (the
  fallback-promotion behavior is a property of how sub-routers merge, not of how
  individual routes are decorated) and spreads the same risk across more call sites
  instead of consolidating it.

## Consequences

- **Two router-building functions must be kept mutually exclusive.** Every new route
  needs an explicit decision about which function it belongs to; there is no default or
  fallback grouping.
- **The top-level `.fallback()` must stay on the outermost, fully-merged router.** Moving
  it onto either `public_router()` or `authenticated_router()` individually would
  reintroduce the original bug for at least one of the two sub-router groups.
- **`app()` is the only sanctioned way to build the full router.** Both `main.rs` and
  integration tests use it so that tests exercise the same wiring (routes + middleware)
  that runs in production; constructing a router by merging the two functions differently
  elsewhere would bypass this guarantee.
