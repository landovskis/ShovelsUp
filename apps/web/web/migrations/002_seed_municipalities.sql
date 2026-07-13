-- REQ-001: seed municipalities fixture data (IMP-REQ-001-06)
-- Domains are best-known official patterns, NOT yet legally reviewed.
-- Do not deploy ingestion in production until legal sign-off lands.
INSERT INTO municipalities (name, slug, domain_allowlist, calendar_url) VALUES
    ('Toronto', 'toronto', ARRAY['toronto.ca'], NULL), -- ASSUMED pending legal review
    ('Vancouver', 'vancouver', ARRAY['vancouver.ca'], NULL), -- ASSUMED pending legal review
    ('Montreal', 'montreal', ARRAY['montreal.ca'], NULL); -- ASSUMED pending legal review
