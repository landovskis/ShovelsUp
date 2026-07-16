-- IMP-REQ-004-01: add the immutable "date surfaced" column to
-- public_search_documents.
--
-- Migration numbering note: 018 (public_search_bilingual, REQ-003) is the
-- latest migration that has landed. 016 still does not exist in this tree
-- (REQ-013's migration hasn't shipped in this execution order), so there is
-- no collision; 019 is the actual next free number.
--
-- Semantics (per tc_004_4_first_surfaced_at_is_immutable_across_refreshes in
-- apps/web/web/tests/search_integration.rs): first_surfaced_at must be set
-- once, on the row's first INSERT into public_search_documents, and must
-- never be touched again by subsequent refresh-job UPSERTs of the same
-- project. This migration only adds the column (nullable, no default, no
-- backfill) and a supporting btree index for REQ-007's later date-range
-- filtering. Wiring the refresh job's INSERT path to actually populate this
-- column is IMP-REQ-004-02's job, not this task's.
ALTER TABLE public_search_documents ADD COLUMN first_surfaced_at TIMESTAMPTZ;

CREATE INDEX idx_public_search_documents_first_surfaced_at
    ON public_search_documents (first_surfaced_at);
