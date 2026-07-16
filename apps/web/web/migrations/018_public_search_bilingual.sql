-- IMP-REQ-003-02: bilingual full-text search support for
-- public_search_documents.
--
-- Migration numbering note (IMP-REQ-003-01 discovery): 017
-- (public_search_municipality_slug, REQ-002) is the latest migration that
-- has landed. 016 does not exist in this tree yet — REQ-013's migration
-- hasn't shipped in this execution order — so there is no collision; 018 is
-- the actual next free number.
--
-- FTS config check (performed live against the local docker-compose Postgres
-- 16-alpine instance per the plan's risk note, via
-- `SELECT cfgname FROM pg_ts_config;`): both `french` and `english` text
-- search configurations ARE available out of the box in this Postgres
-- image (29 configs total, including arabic/danish/dutch/... — `french` and
-- `english` are both present). No environment gap here — the risk the plan
-- flagged did not materialize, so this migration uses the real `french`/
-- `english` configs directly rather than a `simple`+`unaccent` fallback.

-- source_language: nullable, best-effort backfilled below from
-- document_chunks.language (populated at parse time by
-- pipeline::parser::lang::detect_language — see
-- apps/web/pipeline/src/parser/lang.rs and orchestrate.rs). Full ongoing
-- maintenance of this column on every write is IMP-REQ-003-04's job
-- (route/job wiring); this migration only adds the column and performs a
-- one-time historical backfill.
ALTER TABLE public_search_documents ADD COLUMN source_language TEXT;

-- Backfill source_language from each project's most recently created
-- mention's source chunk language, mirroring the exact "latest mention wins"
-- join pattern already used by refresh_public_search_index
-- (apps/web/web/src/jobs/public_search_refresh.rs) for normalized_status.
-- Left as NULL wherever no mention/chunk carries a recorded language (chunk
-- language detection returned "neither language" — see
-- pipeline::parser::lang::detect_language's documented None case) rather
-- than guessing.
UPDATE public_search_documents psd
SET source_language = latest.language
FROM (
    SELECT DISTINCT ON (pm.project_id) pm.project_id, dc.language
    FROM project_mentions pm
    JOIN document_chunks dc ON dc.id = pm.document_chunk_id
    ORDER BY pm.project_id, pm.created_at DESC
) latest
WHERE psd.project_id = latest.project_id;

-- search_vector_fr / search_vector_en: generated (STORED) columns kept
-- automatically in sync by Postgres itself, so no application code has to
-- remember to update them on every write.
--
-- Source text choice: public_search_documents has no dedicated
-- document-body-text column at this layer (confirmed against the live
-- schema — migration 013 only carries civic_address_normalized,
-- municipality_name, project_type, normalized_status as free text, and
-- 017 only added municipality_slug). Until a richer body-text field exists,
-- both vectors are built from the concatenation of
-- civic_address_normalized and municipality_name — the only natural-language
-- fields currently on this table, and already what run_search's ILIKE query
-- (apps/web/web/src/routes/search.rs) matches against today. This makes the
-- FTS columns a same-content, better-ranked upgrade path over the existing
-- ILIKE search rather than a coincidentally different search surface.
ALTER TABLE public_search_documents
    ADD COLUMN search_vector_fr TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('french', coalesce(civic_address_normalized, '') || ' ' || coalesce(municipality_name, ''))
    ) STORED;

ALTER TABLE public_search_documents
    ADD COLUMN search_vector_en TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('english', coalesce(civic_address_normalized, '') || ' ' || coalesce(municipality_name, ''))
    ) STORED;

CREATE INDEX idx_public_search_documents_search_vector_fr
    ON public_search_documents USING gin (search_vector_fr);

CREATE INDEX idx_public_search_documents_search_vector_en
    ON public_search_documents USING gin (search_vector_en);
