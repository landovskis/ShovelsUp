-- IMP-REQ-004-02: seed EN status vocabulary v1 (>=2 synonyms per value)
INSERT INTO status_vocabulary (language, phrase, normalized_status) VALUES
    ('en', 'proposed', 'proposed'),
    ('en', 'submitted', 'proposed'),
    ('en', 'approved', 'approved'),
    ('en', 'adopted', 'approved'),
    ('en', 'deferred', 'deferred'),
    ('en', 'postponed', 'deferred'),
    ('en', 'referred', 'referred'),
    ('en', 'referred to committee', 'referred'),
    ('en', 'rejected', 'rejected'),
    ('en', 'denied', 'rejected');
