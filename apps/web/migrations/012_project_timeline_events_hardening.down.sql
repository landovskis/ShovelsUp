DROP INDEX idx_project_timeline_events_project_event_date_created_at;

ALTER TABLE project_timeline_events
    ALTER COLUMN project_id DROP NOT NULL,
    ALTER COLUMN project_mention_id DROP NOT NULL,
    ALTER COLUMN event_date DROP NOT NULL,
    ALTER COLUMN created_at DROP NOT NULL;
