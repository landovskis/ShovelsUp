-- REQ-004: status_vocabulary + project_mentions.normalized_status (IMP-REQ-004-01)
CREATE TABLE status_vocabulary (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    language TEXT NOT NULL CHECK (language IN ('en', 'fr')),
    phrase TEXT NOT NULL,
    normalized_status TEXT NOT NULL
        CHECK (normalized_status IN ('proposed', 'approved', 'deferred', 'referred', 'rejected')),
    UNIQUE (language, phrase)
);

ALTER TABLE project_mentions
    ADD COLUMN normalized_status TEXT
        CHECK (normalized_status IN ('proposed', 'approved', 'deferred', 'referred', 'rejected'));

-- REQ-004/005/009 share this table (see Implementation Plan coordination
-- note): whichever requirement's migration lands first creates it; the
-- others only add missing columns. REQ-004 needs it now for status-conflict
-- flags; REQ-005 (ambiguous match) and REQ-009 (review queue: version,
-- due_at, audit) will extend it, not recreate it.
CREATE TABLE review_candidates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    candidate_type TEXT NOT NULL,
    project_mention_id UUID REFERENCES project_mentions(id),
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'confirmed', 'rejected')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_review_candidates_project_mention ON review_candidates(project_mention_id);
