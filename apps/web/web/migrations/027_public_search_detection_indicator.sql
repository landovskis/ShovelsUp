-- IMP-REQ-015-02: adds `public_search_documents.first_detected_at` and
-- `.source_count`, the two columns behind the search-card/detail-page
-- confidence indicator ("Detected N day(s) ago from M council source(s)",
-- IMP-REQ-015-06/-07/-11/-12).
--
-- `first_detected_at` vs. `first_surfaced_at` (migration 019): these are
-- deliberately kept as two separate columns even though today's single
-- ingestion path (`refresh_public_search_index`'s `INSERT ... ON CONFLICT
-- DO UPDATE`, which never touches either column past the first insert)
-- populates them identically. `first_surfaced_at` is "when this project
-- first appeared on the public search index" (an index/presentation
-- concept — REQ-004's own date-range filter reads it). `first_detected_at`
-- is "when our pipeline first detected this project from a council source"
-- (a provenance/confidence concept — REQ-015's own indicator). The two
-- concepts coincide today because there is only one path into
-- `public_search_documents`, but they are not the same concept, and a
-- future ingestion path (e.g. a manually curated/backfilled project, or a
-- project re-surfaced after being temporarily hidden) could make them
-- diverge. Duplicating the column now avoids a silent, undocumented reuse
-- of `first_surfaced_at` under a second meaning.
--
-- `source_count` mirrors `latest_meeting_date`'s own precedent (migration
-- 024): a denormalized, materializer-maintained column rather than a
-- per-request join, re-derived (not frozen like `first_surfaced_at`/
-- `first_detected_at`) on every refresh from the current
-- `COUNT(DISTINCT document_chunks.source_document_id)` across every
-- `project_mentions` row resolved to the project (IMP-REQ-015-01's
-- "distinct source document" signal) — the cleanest available "distinct
-- council source" identifier; `document_chunk_id` itself would only be
-- needed as a fallback if `document_chunks.source_document_id` didn't
-- exist, which it does (migration 001).
--
-- Both nullable: a project materialized before this migration (or before
-- IMP-REQ-015-04's materializer logic ran on it at least once) has neither
-- until the next refresh backfills it — the indicator (IMP-REQ-015-06/-07)
-- omits itself entirely when either is `NULL` (TC-015-5), same pattern as
-- `latest_meeting_date`'s own `NULL` handling.
ALTER TABLE public_search_documents
    ADD COLUMN first_detected_at TIMESTAMPTZ,
    ADD COLUMN source_count BIGINT;

-- Mirrors `first_surfaced_at`'s own supporting index (migration 019): not
-- used for sorting today, but the indicator's underlying date is a natural
-- future range-filter/sort candidate, same rationale as that column.
CREATE INDEX idx_public_search_documents_first_detected_at
    ON public_search_documents (first_detected_at);
