-- IMP-REQ-004-03: seed FR status vocabulary v1 (>=2 synonyms per value)
INSERT INTO status_vocabulary (language, phrase, normalized_status) VALUES
    ('fr', 'proposé', 'proposed'),
    ('fr', 'soumis', 'proposed'),
    ('fr', 'approuvé', 'approved'),
    ('fr', 'adopté', 'approved'),
    ('fr', 'reporté', 'deferred'),
    ('fr', 'différé', 'deferred'),
    ('fr', 'référé', 'referred'),
    ('fr', 'renvoyé au comité', 'referred'),
    ('fr', 'rejeté', 'rejected'),
    ('fr', 'refusé', 'rejected');
