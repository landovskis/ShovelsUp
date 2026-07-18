-- IMP-REQ-009-01: adds `public_search_documents.latest_meeting_date`, the
-- column `run_search`'s `sort=date` ordering (IMP-REQ-009-06) sorts on —
-- "the most recent council meeting date this project has been discussed at",
-- backfilled/maintained from `MAX(project_timeline_events.event_date)` for
-- each project, mirroring `category_code`'s own precedent (migration 023):
-- a denormalized column on the public search index, kept in sync by
-- `refresh_public_search_index` (web/src/jobs/public_search_refresh.rs) via
-- a `LEFT JOIN LATERAL` over `project_timeline_events`, rather than joining
-- back to that table on every search request.
--
-- Nullable: a project with no timeline events yet (none of its mentions have
-- been resolved into a `project_timeline_events` row) has no
-- `latest_meeting_date` at all — `sort=date` (TC-009-3) sorts those NULLs
-- last via `NULLS LAST`, never dropping the row from the result set.
--
-- Indexed DESC to serve `ORDER BY latest_meeting_date DESC NULLS LAST`
-- directly, matching migration 012's own precedent of indexing timeline
-- data in the order it's actually queried.
ALTER TABLE public_search_documents
    ADD COLUMN latest_meeting_date TIMESTAMPTZ;

CREATE INDEX idx_public_search_documents_latest_meeting_date
    ON public_search_documents (latest_meeting_date DESC NULLS LAST);
