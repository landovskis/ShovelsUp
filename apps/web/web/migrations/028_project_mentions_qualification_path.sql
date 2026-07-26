ALTER TABLE project_mentions
    ADD COLUMN qualification_path TEXT NOT NULL DEFAULT 'physical_work'
    CHECK (qualification_path IN ('physical_work', 'infrastructure_land_acquisition'));
