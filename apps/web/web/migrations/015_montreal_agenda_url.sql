-- IMP-REQ-001-11: worker needs a real URL to start from. Montreal's actual
-- document index (found by following the link from the marketing page at
-- montreal.ca/conseils-decisionnels/conseil-municipal) is this legacy
-- portal page — verified reachable and containing real typeDoc=pv links via
-- direct curl, 2026-07-11. Toronto/Vancouver stay NULL (documented gap, see
-- docs/superpowers/specs/2026-07-11-fetch-job-worker-design.md Non-goals).
ALTER TABLE municipalities ADD COLUMN agenda_url TEXT;

UPDATE municipalities
SET agenda_url = 'https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL'
WHERE slug = 'montreal';
