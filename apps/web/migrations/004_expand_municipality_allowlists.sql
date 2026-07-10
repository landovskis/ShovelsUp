-- REQ-001 gap-fix: real agenda/minutes documents live on subdomains and
-- sibling domains the original placeholder allowlist missed. Confirmed via
-- research (2026-07-10) against each city's public meeting-calendar system:
--   Toronto   — TMMIS at app.toronto.ca (legacy agendas under toronto.ca/legdocs)
--   Vancouver — covapp.vancouver.ca (interactive council-meetings portal)
--   Montreal  — ville.montreal.qc.ca (agenda PDFs) and the S3-backed asset
--               host portail-m4s.s3.montreal.ca (session PDFs)
-- None of the three exposes an iCal/RSS/JSON feed of meetings — all require
-- scraping an HTML calendar page. Still ASSUMED pending legal review; see
-- docs/runbooks/data_pipeline_ingestion.md.
UPDATE municipalities SET domain_allowlist = ARRAY['toronto.ca', 'app.toronto.ca']
    WHERE slug = 'toronto';
UPDATE municipalities SET domain_allowlist = ARRAY['vancouver.ca', 'covapp.vancouver.ca']
    WHERE slug = 'vancouver';
UPDATE municipalities SET domain_allowlist = ARRAY['montreal.ca', 'ville.montreal.qc.ca', 'portail-m4s.s3.montreal.ca']
    WHERE slug = 'montreal';
