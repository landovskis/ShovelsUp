-- IMP-REQ-005-01: additive, nullable columns for the project detail page
-- (GET /projects/{id}). Discovery (IMP-REQ-005-02) confirmed no per-
-- municipality schema variant exists (Toronto/Vancouver/Montreal share the
-- same projects/project_mentions/source_documents tables) and no existing
-- "confidence" or free-text "description" signal exists anywhere in
-- apps/web/pipeline/src/ (grepped both terms; only hits were the
-- extraction prompt's own JSON-schema field doc-strings, not a data
-- column) — so all three columns below are genuinely new concepts with no
-- backfill source, left NULL until a future requirement populates them.
--
-- description_lang: language of a future extracted description field
-- (none exists yet) — tracked ahead of time per the plan's graceful-
-- degradation design (IMP-REQ-005-05/06 render "no description" when
-- NULL, not an error).
-- confidence_level: no existing pipeline signal defines a value set, so
-- left as unconstrained TEXT rather than a CHECK against a set this
-- migration would have to guess at.
-- source_document_url: a denormalized copy of source_documents.source_url
-- for the detail page's convenience (avoids an extra JOIN on every page
-- load); populating it is IMP-REQ-005-03/05's job (handler/context-struct
-- wiring), not this migration.
ALTER TABLE projects
    ADD COLUMN description_lang TEXT,
    ADD COLUMN confidence_level TEXT,
    ADD COLUMN source_document_url TEXT;
