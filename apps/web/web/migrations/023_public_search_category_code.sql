-- IMP-REQ-008-04/-05: wires `category_taxonomy` (migration 022) into the
-- public search path.
--
-- 1. `category_taxonomy.sort_order`: migration 022 seeded the five codes in
--    a specific, deliberate order (residential, commercial, institutional,
--    infrastructure, other — matching TC-008-4's exact expected list), but
--    relied on implicit insertion/physical order, which Postgres never
--    guarantees for a plain `SELECT` without an `ORDER BY`. `GET
--    /categories` (IMP-REQ-008-04) needs a deterministic, explicit ordering
--    column to sort by rather than depending on incidental heap layout.
--
-- 2. `public_search_documents.category_code`: the denormalized public
--    search index (migration 013) has no category information at all.
--    `projects.category_code` (migration 022) is the source of truth per
--    project; this column mirrors it onto the search index the same way
--    `municipality_slug`/`project_type`/`normalized_status` are already
--    mirrored, so `run_search` (IMP-REQ-008-04/-05) can filter on it
--    directly without joining back to `projects` on every search request.
--    `refresh_public_search_index` (web/src/jobs/public_search_refresh.rs)
--    is updated in the same change to keep it populated on every
--    insert/update, matching that job's existing pattern for every other
--    mirrored column.
ALTER TABLE category_taxonomy
    ADD COLUMN sort_order INT NOT NULL DEFAULT 0;

UPDATE category_taxonomy SET sort_order = 1 WHERE code = 'residential';
UPDATE category_taxonomy SET sort_order = 2 WHERE code = 'commercial';
UPDATE category_taxonomy SET sort_order = 3 WHERE code = 'institutional';
UPDATE category_taxonomy SET sort_order = 4 WHERE code = 'infrastructure';
UPDATE category_taxonomy SET sort_order = 5 WHERE code = 'other';

ALTER TABLE public_search_documents
    ADD COLUMN category_code TEXT REFERENCES category_taxonomy(code);
