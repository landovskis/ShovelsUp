-- IMP-REQ-008-02: category_taxonomy lookup table + nullable
-- projects.category_code, backing the `category` search filter
-- (TC-008-1/-2/-3) and the `GET /categories` facet endpoint (TC-008-4/-5).
--
-- Discovery (IMP-REQ-008-01) confirmed the extraction pipeline's
-- `project_type` (see pipeline/src/extractor/schema.rs,
-- pipeline/src/extractor/prompts/en.rs and fr.rs) is free-text produced by
-- the LLM, not a fixed enum: the prompt only offers examples ("e.g.
-- residential, commercial, mixed-use, institutional, infrastructure,
-- industrial") and the model is free to return anything reasonably
-- inferable, including French raw values ("résidentiel", "institutionnel")
-- observed in pipeline/src/extractor/mod.rs's own test fixtures.
-- web/src/routes/search.rs:611-612 already documents this explicitly:
-- "project_type is free text from the extraction pipeline (no fixed enum
-- backs it)".
--
-- `category_taxonomy` is therefore a NEW, separate, coarse-grained
-- controlled vocabulary for the public-facing `category` filter/facet — it
-- is not a direct copy of the pipeline's free-text `project_type` values.
-- Mapping a project's raw `project_type` text onto one of these five codes
-- (or leaving `category_code` NULL, i.e. "uncategorised") is a future
-- classification task's job, not this migration's. The five codes below
-- match exactly what tests/search_integration.rs's TC-008-4
-- (`categories_facet_endpoint_returns_full_taxonomy`) asserts as the
-- complete taxonomy list, in that order: residential, commercial,
-- institutional, infrastructure, other. "other" has no pipeline analogue
-- (the pipeline never emits it) — it exists purely as this taxonomy's
-- catch-all bucket for projects that don't fit the first four buckets.
-- "mixed-use" and "industrial", which the pipeline's prompt does offer as
-- project_type examples, are deliberately NOT separate taxonomy codes here;
-- future classification logic can fold them into the closest bucket (or
-- "other") when it exists.
--
-- All five seeded codes are public (`is_public = true`, the column
-- default) — nothing in the plan or TC-008-* calls for a hidden/internal
-- category yet, but the column exists so a future non-public code doesn't
-- require a schema change.
CREATE TABLE category_taxonomy (
    code TEXT PRIMARY KEY,
    label_en TEXT NOT NULL,
    label_fr TEXT NOT NULL,
    is_public BOOLEAN NOT NULL DEFAULT true
);

INSERT INTO category_taxonomy (code, label_en, label_fr) VALUES
    ('residential', 'Residential', 'Résidentiel'),
    ('commercial', 'Commercial', 'Commercial'),
    ('institutional', 'Institutional', 'Institutionnel'),
    ('infrastructure', 'Infrastructure', 'Infrastructures'),
    ('other', 'Other', 'Autre');

-- Nullable, no backfill: nothing currently classifies existing projects
-- into this taxonomy (that's a future requirement's job). NULL is the
-- real "uncategorised" state matched by the `category=uncategorised`
-- pseudo-value (TC-008-2).
ALTER TABLE projects
    ADD COLUMN category_code TEXT REFERENCES category_taxonomy(code);
