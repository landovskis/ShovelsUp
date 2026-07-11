-- IMP-REQ-009-02: review_candidates/audit_events migrations, coordinated
-- with REQ-005 which already created review_candidates (candidate_type,
-- project_mention_id, details, status, created_at). This migration only
-- adds the columns REQ-009's confirm/reject workflow needs — it does not
-- re-create the table, per the plan's stated migration-ownership rule.
-- `due_at`'s column DEFAULT is a naive calendar-day fallback that only
-- applies to rows inserted without an explicit value (i.e. backfilling
-- review_candidates rows that predate this migration). Application code
-- (resolver::try_resolve) always computes and passes an explicit due_at
-- via business_days::add_business_days (IMP-REQ-009-01), which is
-- weekday-aware — the two are not the same value in general.
ALTER TABLE review_candidates
    ADD COLUMN version INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN due_at TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '2 days'),
    ADD COLUMN resolved_project_id UUID REFERENCES projects(id);

CREATE INDEX idx_review_candidates_status_due_at ON review_candidates(status, due_at);

-- Records every confirm/reject action for operator accountability.
CREATE TABLE audit_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    review_candidate_id UUID NOT NULL REFERENCES review_candidates(id),
    action TEXT NOT NULL CHECK (action IN ('confirm', 'reject')),
    actor TEXT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_audit_events_review_candidate ON audit_events(review_candidate_id);
