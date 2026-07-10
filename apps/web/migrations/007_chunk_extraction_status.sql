-- REQ-003: track per-chunk extraction outcome. TC-REQ-003-4 requires a
-- malformed-JSON chunk be marked failed (not just silently skipped) and
-- TC-REQ-003-5/IMP-REQ-003-06 requires a retryable transient-failure state,
-- mirroring source_documents.parser_status from REQ-002.
ALTER TABLE document_chunks
    ADD COLUMN extraction_status TEXT NOT NULL DEFAULT 'pending'
        CHECK (extraction_status IN ('pending', 'extracted', 'no_mention', 'failed', 'reprocessing'));
