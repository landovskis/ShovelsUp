-- REQ-006: preserve the resolver's optional normalized_status while making
-- timeline ownership and ordering fields explicitly required.
ALTER TABLE project_timeline_events
    ALTER COLUMN project_id SET NOT NULL,
    ALTER COLUMN project_mention_id SET NOT NULL,
    ALTER COLUMN event_date SET NOT NULL,
    ALTER COLUMN created_at SET NOT NULL;

-- Serves chronological project-timeline reads, including their stable
-- ingestion-time tie-breaker.
CREATE INDEX idx_project_timeline_events_project_event_date_created_at
    ON project_timeline_events (project_id, event_date, created_at);
