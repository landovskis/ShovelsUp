-- IMP-REQ-013-01: `merged_into_id` lets one project be marked as merged into
-- another (canonical) project, so a stale/duplicate project's shareable URL
-- can 301-redirect to the survivor instead of rendering its own (now
-- superseded) page (TC-013-1). Nullable: most projects are never merged.
--
-- Self-referencing FK to `projects.id`. `ON DELETE SET NULL` rather than
-- `CASCADE`/`RESTRICT`: if the canonical target project is ever deleted, the
-- merged project should fall back to rendering its own page again rather
-- than either cascading into an unrelated delete or blocking the delete
-- entirely.
--
-- One-hop CHECK: enforced via a trigger, not a plain column CHECK constraint
-- (a CHECK on a single row can't see other rows' data, and "does my target
-- itself have merged_into_id set" requires a lookup against another row).
-- This keeps the invariant "a merge chain is never more than one hop deep"
-- (A -> B is fine; A -> B -> C is not) enforced at the database level rather
-- than trusted to application code alone.
ALTER TABLE projects
    ADD COLUMN merged_into_id UUID REFERENCES projects(id) ON DELETE SET NULL;

CREATE INDEX idx_projects_merged_into_id ON projects (merged_into_id) WHERE merged_into_id IS NOT NULL;

CREATE OR REPLACE FUNCTION enforce_projects_merged_into_one_hop()
RETURNS TRIGGER AS $$
DECLARE
    target_already_merged BOOLEAN;
    row_is_already_a_merge_target BOOLEAN;
BEGIN
    IF NEW.merged_into_id IS NULL THEN
        RETURN NEW;
    END IF;

    IF NEW.merged_into_id = NEW.id THEN
        RAISE EXCEPTION 'projects.merged_into_id cannot reference itself (id=%)', NEW.id;
    END IF;

    -- Forward check: the target this row is merging into must not itself
    -- already be merged elsewhere (would create A -> target -> elsewhere).
    SELECT (merged_into_id IS NOT NULL) INTO target_already_merged
    FROM projects
    WHERE id = NEW.merged_into_id;

    IF target_already_merged THEN
        RAISE EXCEPTION 'projects.merged_into_id must be one-hop: target project % is itself merged into another project', NEW.merged_into_id;
    END IF;

    -- Reverse check: this row must not already be the merge target of some
    -- other project (would retroactively create other -> this -> NEW.merged_into_id).
    SELECT EXISTS (
        SELECT 1 FROM projects WHERE merged_into_id = NEW.id
    ) INTO row_is_already_a_merge_target;

    IF row_is_already_a_merge_target THEN
        RAISE EXCEPTION 'projects.merged_into_id must be one-hop: project % is already the merge target of another project', NEW.id;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_projects_merged_into_one_hop
    BEFORE INSERT OR UPDATE OF merged_into_id ON projects
    FOR EACH ROW
    EXECUTE FUNCTION enforce_projects_merged_into_one_hop();
