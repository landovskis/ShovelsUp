-- REQ-002 prerequisite fix: REQ-001's Fetcher computed a checksum and
-- discarded the fetched body, but nothing in source_documents actually
-- stores document content for REQ-002's parsers to read — and REQ-001's
-- Fetcher additionally decoded every response as UTF-8 text via
-- reqwest::Response::text(), which corrupts binary PDF bytes. Both are
-- fixed together here: raw bytes are now persisted and read as bytes
-- throughout the fetch path.
ALTER TABLE source_documents
    ADD COLUMN content BYTEA NOT NULL;
