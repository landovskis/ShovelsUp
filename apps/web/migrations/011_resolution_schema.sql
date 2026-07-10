-- REQ-005: projects + project_timeline_events + resolution linkage
-- (IMP-REQ-005-01). project_timeline_events is created here (not REQ-006)
-- because REQ-005's resolver is the writer; REQ-006 (IMP-REQ-006-01) only
-- hardens it with NOT NULL/index constraints per the plan's own sequencing.
CREATE TABLE projects (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    civic_address_normalized TEXT,
    project_type TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Rejects a duplicate (address, type) insert outside the ambiguous
-- (review-candidate) state, per IMP-REQ-005-01's acceptance criteria.
CREATE UNIQUE INDEX idx_projects_address_type_unique
    ON projects(civic_address_normalized, project_type)
    WHERE civic_address_normalized IS NOT NULL AND project_type IS NOT NULL;

ALTER TABLE project_mentions
    ADD COLUMN project_id UUID REFERENCES projects(id),
    -- Explicit cross-reference cue (e.g. "Application No. 2026-045") that
    -- REQ-003's original schema didn't capture — added here because
    -- RULE-003's "explicit cross-reference" matcher needs something
    -- concrete to match on; extending REQ-003's extraction schema rather
    -- than inventing a proxy signal.
    ADD COLUMN reference_number TEXT;

CREATE TABLE project_timeline_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id),
    project_mention_id UUID NOT NULL REFERENCES project_mentions(id),
    -- No per-agenda-item event date is captured anywhere upstream yet (see
    -- REQ-004's documented conflict-resolution limitation) — event_date
    -- defaults to the mention's extraction time as an interim proxy until
    -- real meeting-item dates are available.
    event_date TIMESTAMPTZ NOT NULL DEFAULT now(),
    normalized_status TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_project_timeline_events_project ON project_timeline_events(project_id);
