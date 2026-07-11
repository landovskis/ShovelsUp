-- IMP-REQ-008-01: denormalized public search index. Deliberately separate
-- from project_mentions/projects (the internal/admin schema) so PII and
-- internal fields (reference numbers, raw LLM output, review state) are
-- structurally impossible to leak through the public search API — this
-- table only ever contains the subset of fields safe to expose
-- unauthenticated.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE public_search_documents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL UNIQUE REFERENCES projects(id),
    civic_address_normalized TEXT NOT NULL,
    municipality_name TEXT,
    project_type TEXT,
    normalized_status TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_public_search_documents_address_trgm
    ON public_search_documents USING gin (civic_address_normalized gin_trgm_ops);

CREATE INDEX idx_public_search_documents_municipality_trgm
    ON public_search_documents USING gin (municipality_name gin_trgm_ops);

CREATE INDEX idx_public_search_documents_municipality_btree
    ON public_search_documents (municipality_name);
