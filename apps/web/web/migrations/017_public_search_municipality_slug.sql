-- IMP-REQ-002-01: add a municipality_slug column to public_search_documents so
-- public search results can link/filter by the stable municipality slug
-- (municipalities.slug) instead of the free-text municipality_name. Backfilled
-- by joining on municipalities.name, which is the only linkage available on
-- public_search_documents today (it stores denormalized municipality_name,
-- not municipality_id — see migration 013).
ALTER TABLE public_search_documents ADD COLUMN municipality_slug TEXT;

UPDATE public_search_documents
SET municipality_slug = municipalities.slug
FROM municipalities
WHERE public_search_documents.municipality_name = municipalities.name;

-- Parity check: the backfill join above silently leaves municipality_slug
-- NULL if a document's municipality_name doesn't match any municipalities.name
-- (e.g. drift between denormalized text and the source-of-truth table). Fail
-- the migration rather than let that pass unnoticed, per the plan's risk note.
DO $$
DECLARE
    mismatched_count INTEGER;
BEGIN
    SELECT count(*) INTO mismatched_count
    FROM public_search_documents
    WHERE municipality_name IS NOT NULL
      AND municipality_slug IS NULL;

    IF mismatched_count > 0 THEN
        RAISE EXCEPTION 'municipality_slug backfill left % row(s) NULL despite a non-NULL municipality_name (name/slug mismatch against municipalities table)', mismatched_count;
    END IF;
END $$;

-- Plain btree index for equality lookups (e.g. filtering search results by
-- municipality slug); not unique, since many documents share a municipality.
CREATE INDEX idx_public_search_documents_municipality_slug
    ON public_search_documents (municipality_slug);
