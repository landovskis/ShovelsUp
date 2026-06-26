# Data Model: Council Meeting Notes Import & Construction Tracking

**Feature**: specs/001-council-meeting-import/spec.md
**Date**: 2026-06-25
**Storage**: SQLite via sqlx, WAL mode

---

## Entities

### Meeting

Represents a single City Council (conseil municipal) session. Created when the portal crawler discovers a new PDF link on the HTML index. Each meeting has exactly one source PDF.

| Field | Type | Constraints | Notes |
|-------|------|-------------|-------|
| `id` | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal identifier |
| `reference_number` | TEXT | NOT NULL, UNIQUE | City-assigned meeting reference from the portal |
| `meeting_date` | DATE | NOT NULL | Date of the council session |
| `source_url` | TEXT | NOT NULL | URL of the HTML portal page where this meeting was discovered |
| `pdf_url` | TEXT | NOT NULL | Direct URL of the PDF agenda document |
| `import_status` | TEXT | NOT NULL, CHECK | `pending` → `processing` → `imported` or `failed` |
| `item_count` | INTEGER | NULLABLE | Number of construction items extracted (NULL until imported) |
| `error_message` | TEXT | NULLABLE | Set on `failed` status |
| `imported_at` | TIMESTAMP | NULLABLE | Set when status reaches `imported` |
| `created_at` | TIMESTAMP | NOT NULL, DEFAULT NOW | When the meeting was first discovered |

**State transitions**: `pending` → `processing` → `imported` | `failed`. A failed meeting may be retried (reset to `pending`).

---

### ConstructionProject

A tracked construction or development project. Identity is determined by `dossier_number` (when present) or `normalized_address` (fallback). One project may be linked to decisions across multiple meetings.

| Field | Type | Constraints | Notes |
|-------|------|-------------|-------|
| `id` | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal identifier |
| `dossier_number` | TEXT | UNIQUE, NULLABLE | City-assigned permit/dossier reference; NULL if not found in agenda text |
| `normalized_address` | TEXT | NOT NULL | Canonical form: `{number} {street name}, {borough}` |
| `borough` | TEXT | NOT NULL | Montreal arrondissement name (in French, as found in agenda) |
| `project_type` | TEXT | NOT NULL, CHECK | `permit` \| `zoning` \| `demolition` \| `development` |
| `current_status` | TEXT | NOT NULL, CHECK | Most recent decision: `approved` \| `deferred` \| `rejected` \| `amended` \| `pending` |
| `created_at` | TIMESTAMP | NOT NULL, DEFAULT NOW | When first discovered |
| `updated_at` | TIMESTAMP | NOT NULL, DEFAULT NOW | Updated on each new decision |

**Identity rule**: When importing an agenda item, the system first searches for a matching `dossier_number`. If no dossier number is present, it searches by `normalized_address`. If no match exists, a new project record is created.

**Indexes**: `borough`, `current_status`, `normalized_address` (for filtering and search).

---

### ProjectDecision

A council decision for a project at a specific meeting. A project accumulates one `ProjectDecision` per meeting in which it appears, forming its timeline.

| Field | Type | Constraints | Notes |
|-------|------|-------------|-------|
| `id` | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal identifier |
| `project_id` | INTEGER | NOT NULL, FK → ConstructionProject | The project this decision concerns |
| `meeting_id` | INTEGER | NOT NULL, FK → Meeting | The meeting at which this decision was made |
| `decision_type` | TEXT | NOT NULL, CHECK | `approved` \| `deferred` \| `rejected` \| `amended` |
| `conditions` | TEXT | NULLABLE | Any attached conditions or amendments (in French) |
| `raw_description_fr` | TEXT | NOT NULL | Original French agenda text for this item |
| `decided_at` | DATE | NOT NULL | Meeting date (denormalized for query convenience) |
| `created_at` | TIMESTAMP | NOT NULL, DEFAULT NOW | When this record was created |

**Constraint**: UNIQUE on `(project_id, meeting_id)` — a project appears at most once per meeting.

---

### ImportLog

Auditable record of every import attempt, successful or not. Drives the admin dashboard and freshness alerting.

| Field | Type | Constraints | Notes |
|-------|------|-------------|-------|
| `id` | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal identifier |
| `meeting_id` | INTEGER | NULLABLE, FK → Meeting | NULL if failure occurred before meeting record was created |
| `attempt_at` | TIMESTAMP | NOT NULL | When the import attempt started |
| `outcome` | TEXT | NOT NULL, CHECK | `success` \| `failure` \| `no_items` |
| `items_extracted` | INTEGER | NOT NULL, DEFAULT 0 | Construction items found (0 on failure or no_items) |
| `error_detail` | TEXT | NULLABLE | Error description on failure |
| `duration_ms` | INTEGER | NULLABLE | Wall-clock time for the import |
| `created_at` | TIMESTAMP | NOT NULL, DEFAULT NOW | Record creation time |

---

### ClassificationRule

Configurable rules for identifying construction-related agenda items by section heading or keyword. Editable without a code change.

| Field | Type | Constraints | Notes |
|-------|------|-------------|-------|
| `id` | INTEGER | PRIMARY KEY AUTOINCREMENT | Internal identifier |
| `rule_type` | TEXT | NOT NULL, CHECK | `section_heading` \| `keyword` |
| `pattern` | TEXT | NOT NULL | The heading text or keyword to match (French) |
| `language` | TEXT | NOT NULL, DEFAULT 'fr' | Language of the pattern |
| `enabled` | INTEGER | NOT NULL, DEFAULT 1 | 0 = disabled without deletion |
| `created_at` | TIMESTAMP | NOT NULL, DEFAULT NOW | When added |

**Constraint**: UNIQUE on `(rule_type, pattern)`.

**Default section headings**: `Urbanisme`, `Permis et dérogations`, `Aménagement urbain`, `Réglementation d'urbanisme`

**Default keywords**: `permis de construction`, `dérogation`, `zonage`, `démolition`, `lotissement`, `usage conditionnel`, `résolution de zonage`

---

## Entity Relationship Diagram

```
Meeting ──< ProjectDecision >── ConstructionProject
   │
   └──< ImportLog

ClassificationRule  (independent, loaded by classifier service)
```

- One `Meeting` produces zero or many `ProjectDecision` records
- One `ConstructionProject` accumulates one or many `ProjectDecision` records (its timeline)
- One `Meeting` has zero or many `ImportLog` entries (one per import attempt)
- `ClassificationRule` is read by the classifier service; not directly related to other entities

---

## Migration File: `migrations/001_initial.sql`

```sql
CREATE TABLE meetings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    reference_number TEXT NOT NULL UNIQUE,
    meeting_date DATE NOT NULL,
    source_url TEXT NOT NULL,
    pdf_url TEXT NOT NULL,
    import_status TEXT NOT NULL CHECK (import_status IN ('pending', 'processing', 'imported', 'failed')),
    item_count INTEGER,
    error_message TEXT,
    imported_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE construction_projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    dossier_number TEXT UNIQUE,
    normalized_address TEXT NOT NULL,
    borough TEXT NOT NULL,
    project_type TEXT NOT NULL CHECK (project_type IN ('permit', 'zoning', 'demolition', 'development')),
    current_status TEXT NOT NULL CHECK (current_status IN ('approved', 'deferred', 'rejected', 'amended', 'pending')),
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_projects_borough ON construction_projects(borough);
CREATE INDEX idx_projects_status ON construction_projects(current_status);
CREATE INDEX idx_projects_address ON construction_projects(normalized_address);

CREATE TABLE project_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES construction_projects(id),
    meeting_id INTEGER NOT NULL REFERENCES meetings(id),
    decision_type TEXT NOT NULL CHECK (decision_type IN ('approved', 'deferred', 'rejected', 'amended')),
    conditions TEXT,
    raw_description_fr TEXT NOT NULL,
    decided_at DATE NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (project_id, meeting_id)
);

CREATE TABLE import_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id INTEGER REFERENCES meetings(id),
    attempt_at TIMESTAMP NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'failure', 'no_items')),
    items_extracted INTEGER NOT NULL DEFAULT 0,
    error_detail TEXT,
    duration_ms INTEGER,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE classification_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_type TEXT NOT NULL CHECK (rule_type IN ('section_heading', 'keyword')),
    pattern TEXT NOT NULL,
    language TEXT NOT NULL DEFAULT 'fr',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (rule_type, pattern)
);

INSERT INTO classification_rules (rule_type, pattern) VALUES
    ('section_heading', 'Urbanisme'),
    ('section_heading', 'Permis et dérogations'),
    ('section_heading', 'Aménagement urbain'),
    ('section_heading', 'Réglementation d''urbanisme'),
    ('keyword', 'permis de construction'),
    ('keyword', 'dérogation'),
    ('keyword', 'zonage'),
    ('keyword', 'démolition'),
    ('keyword', 'lotissement'),
    ('keyword', 'usage conditionnel'),
    ('keyword', 'résolution de zonage');
```
