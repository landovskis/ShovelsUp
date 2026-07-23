-- IMP-REQ-014-01: telemetry sink for the project-detail page's non-modal
-- "Get alerts — sign up" upsell CTA card. `POST /api/v1/cta-events` (a fully
-- public, unauthenticated endpoint per REQ-010 — every detail-page view is
-- anonymous) writes one row per fire-and-forget beacon (an "impression" on
-- page load, a "click" when the visitor follows the signup link). This is a
-- write-mostly telemetry table, not a source-of-truth business table — it
-- has no downstream readers in this pass; it exists so growth/product can
-- later query CTA effectiveness.
--
-- `project_id` is a required FK to `projects.id`, `ON DELETE CASCADE`:
-- an event is meaningless once its project is gone, and unlike
-- `projects.merged_into_id` (ADR 010's `SET NULL`, which preserves the
-- merged project's own page), there is no equivalent "fall back to
-- standalone rendering" concern for a telemetry row.
--
-- `event_type` is constrained to the small, fixed set of events the CTA
-- card's own JS (IMP-REQ-014-09) actually emits — `impression` (fired once
-- on page load via `navigator.sendBeacon`) and `click` (fired when the
-- visitor follows the `/signup` link) — via a CHECK constraint, the same
-- "small hand-curated vocabulary" pattern `category_taxonomy` established,
-- rather than an open-ended free-text column that could silently accept
-- typos/garbage from a future caller.
CREATE TABLE cta_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL CHECK (event_type IN ('impression', 'click')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_cta_events_project_id ON cta_events (project_id);
