-- REQ-003: project_mentions with a scale-indicator CHECK (IMP-REQ-003-01)
CREATE TABLE project_mentions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_chunk_id UUID NOT NULL REFERENCES document_chunks(id),
    physical_work BOOLEAN NOT NULL,
    project_name TEXT,
    civic_address TEXT,
    project_type TEXT,
    scale_units INTEGER,
    scale_gfa_sqm DOUBLE PRECISION,
    scale_storeys INTEGER,
    approval_status_raw TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A physical-work mention must carry at least one scale indicator; the
    -- PRD accepts "at least one of units/GFA/storeys", not all three.
    CONSTRAINT scale_indicator_required_for_physical_work CHECK (
        NOT physical_work
        OR scale_units IS NOT NULL
        OR scale_gfa_sqm IS NOT NULL
        OR scale_storeys IS NOT NULL
    )
);

CREATE INDEX idx_project_mentions_document_chunk ON project_mentions(document_chunk_id);
