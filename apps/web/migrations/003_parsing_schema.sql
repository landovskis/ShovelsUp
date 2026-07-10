-- REQ-002: document_chunks + parser tracking columns on source_documents (IMP-REQ-002-01)
ALTER TABLE source_documents
    ADD COLUMN content_type TEXT,
    ADD COLUMN parser_status TEXT NOT NULL DEFAULT 'pending'
        CHECK (parser_status IN ('pending', 'parsed', 'failed', 'reprocessing'));

CREATE TABLE document_chunks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_document_id UUID NOT NULL REFERENCES source_documents(id),
    chunk_index INTEGER NOT NULL,
    content TEXT NOT NULL,
    language TEXT,
    parse_method TEXT NOT NULL DEFAULT 'text' CHECK (parse_method IN ('text', 'ocr')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source_document_id, chunk_index)
);

CREATE INDEX idx_document_chunks_source_document ON document_chunks(source_document_id);
