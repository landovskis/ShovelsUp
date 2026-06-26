# Feature Specification: Council Meeting Notes Import & Construction Tracking

**Feature Branch**: `feature/montreal-import-2`

**Created**: 2026-06-25

**Status**: Draft

**Input**: User description: "Track construction projects based on import of meeting notes from https://ville.montreal.qc.ca/portal/page?_pageid=5798,85945578&_dad=portal&_schema=PORTAL"

## Clarifications

### Session 2026-06-25

- Q: How does the system determine a project in meeting A is the same project as one in meeting B? → A: Hybrid — city-assigned dossier/permit reference number when present, normalized address (street number + street name + borough) as fallback when absent.
- Q: What format are meeting documents published in on the Montreal portal? → A: PDF documents linked from an HTML index page — the system parses the HTML index to discover new meetings, then downloads and parses the linked PDF agendas to extract construction items.
- Q: How does the system alert administrators when the portal is repeatedly unreachable? → A: Email notification to a configured administrator address.
- Q: Where are import logs visible to administrators? → A: Dedicated admin page within the web app showing import history, status, and errors.
- Q: How does the system identify which agenda items are construction-related? → A: Combination of section heading detection (e.g., "Urbanisme", "Permis") and keyword matching against a configurable list of construction/development terms in French — both signals used together.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Browse Recent Construction Decisions (Priority: P1)

A resident or journalist visits the site to see what construction and development projects were recently approved or discussed at City Council meetings.

**Why this priority**: This is the core value proposition — making public meeting data accessible without requiring users to navigate the city's complex portal. It delivers immediate value as soon as any meeting is imported.

**Independent Test**: Can be fully tested by importing one meeting document and verifying users can see the extracted projects in a browseable list.

**Acceptance Scenarios**:

1. **Given** at least one meeting has been imported, **When** a user visits the site, **Then** they see a list of construction projects with key details (address, project type, decision, date) in their preferred language (EN or FR)
2. **Given** a list of construction projects, **When** a user selects a project, **Then** they see the full details including the council decision and meeting reference
3. **Given** the site is displaying in EN, **When** a user switches to FR, **Then** all UI elements and available translated content display in French

---

### User Story 2 - Automatic Discovery and Import of New Meetings (Priority: P2)

The system automatically detects when new meetings are published on the Montreal City Council portal and imports their construction-related content without requiring manual action.

**Why this priority**: The import mechanism feeds all other features. Without it, no data exists to display. Automatic discovery ensures the site stays current without ongoing manual effort.

**Independent Test**: Can be fully tested by simulating a new meeting appearing on the portal and verifying it is discovered, imported, and its projects appear in the list automatically.

**Acceptance Scenarios**:

1. **Given** the system is running, **When** a new meeting document is published on the Montreal portal, **Then** the system detects it and imports its construction-related agenda items without any manual action
2. **Given** a meeting is discovered that has already been imported, **When** the system processes it, **Then** existing project records are updated rather than duplicated
3. **Given** the portal is temporarily unavailable during a scheduled check, **When** the check runs, **Then** the system records the failure, retries at the next scheduled interval, and raises an alert if repeated failures occur
4. **Given** a meeting document is published in an unrecognized format, **When** the system attempts to parse it, **Then** the failure is logged with the source URL so an administrator can investigate

---

### User Story 3 - Filter and Search Projects (Priority: P3)


A journalist or resident wants to find construction projects in a specific area or of a specific type.

**Why this priority**: Filtering and search make the data actionable at scale but are not needed to demonstrate initial value. A single-meeting import still validates the core feature.

**Independent Test**: Can be fully tested by importing multiple meetings and verifying that filtering by borough returns only projects from that borough.

**Acceptance Scenarios**:

1. **Given** multiple construction projects exist, **When** a user filters by borough/arrondissement, **Then** only projects in that borough are shown
2. **Given** a search query, **When** a user searches by address or project description, **Then** matching results appear in under 3 seconds
3. **Given** no projects match the applied filter, **When** a user views the list, **Then** a clear "no results" message is shown with a suggestion to adjust filters

---

### User Story 4 - Monitor Import Health via Admin Dashboard (Priority: P4)

An administrator logs into a protected admin section of the web app to review the history of import attempts, spot failures, and confirm data freshness.

**Why this priority**: The admin dashboard provides operational visibility without requiring server access. It is essential for a non-technical administrator to verify the site is staying current, but not needed before the core browsing experience is functional.

**Independent Test**: Can be fully tested by triggering both a successful and a failed import, logging in as an admin, and verifying both appear in the import log with correct status.

**Acceptance Scenarios**:

1. **Given** an administrator navigates to the admin section, **When** they enter valid credentials, **Then** they are granted access to the import log and denied access without valid credentials
2. **Given** the admin is authenticated, **When** they view the import log, **Then** they see a chronological list of import attempts with date, source URL, outcome (success/failure), and number of items extracted
3. **Given** an import failed, **When** the admin views the log entry, **Then** they see an error description sufficient to diagnose the cause
4. **Given** the admin views the dashboard, **When** the most recent successful import is more than 48 hours old, **Then** a prominent freshness warning is displayed

---

### User Story 5 - Track Project Status Across Meetings (Priority: P5)

A resident wants to follow the progress of a construction project through multiple City Council meetings (e.g., initial proposal → deferral → approval).

**Why this priority**: Tracking over time differentiates this from a simple agenda viewer and adds journalistic value, but requires multiple meetings to be imported first.

**Independent Test**: Can be fully tested by importing two meetings that reference the same project and verifying the project timeline shows both events in order.

**Acceptance Scenarios**:

1. **Given** a project appeared in two different meetings, **When** a user views the project, **Then** they see a chronological timeline of council decisions for that project
2. **Given** a project's status changed between meetings (e.g., deferred → approved), **When** a user views the project, **Then** the current status reflects the most recent decision

---

### Edge Cases

- What happens when a PDF agenda cannot be downloaded (network error, 404)?
- What happens when a downloaded PDF is corrupted, password-protected, or uses a layout the parser cannot interpret?
- What if the HTML index page structure changes and meeting links can no longer be found?
- What if an agenda item is under a known construction section heading but contains no recognizable keywords — is it included or excluded?
- What if the city renames agenda sections (e.g., "Urbanisme" → "Développement urbain") — how are classification rules updated?
- How does the system handle agenda items referencing multiple addresses or an entire street corridor?
- What if a project appears under slightly different descriptions in different meetings (deduplication)? The system uses city dossier number as primary key and normalized address as fallback; an unresolvable conflict (different dossier number, same address) requires manual review.
- What if a project has neither a dossier reference number nor a parseable address? **Decision**: the item is skipped (no ConstructionProject record created) and logged as `unresolvable` in the ImportLog error_detail field so an administrator can investigate.
- How are projects that span multiple boroughs classified?
- What happens if the Montreal portal changes its document structure or URL scheme?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST automatically check the Montreal City Council portal's HTML meeting index on a regular schedule, detect newly linked PDF agenda documents, and queue them for import
- **FR-002**: The system MUST download and parse each newly discovered PDF agenda, identifying construction and development items by combining section heading detection (e.g., "Urbanisme", "Permis et dérogations") with keyword matching against a configurable list of French construction/development terms, then extracting address, borough, dossier/permit reference number (when present), project type, decision outcome, and meeting date from each matched item
- **FR-003**: The system MUST store each extracted construction project as a tracked record whose identity is determined by the city-assigned dossier/permit reference number when present, or by normalized address (street number + street name + borough) when no reference number is available
- **FR-004**: The system MUST detect previously imported meetings and update existing project records rather than creating duplicates, using the identity rule defined in FR-003
- **FR-005**: The system MUST log all import attempts (success/failure, source URL, item count, timestamp) and display them in a protected admin page within the web app
- **FR-013**: The admin section MUST require authentication via HTTP Basic Auth with bcrypt-hashed credentials configured as environment variables; access without valid credentials MUST be denied with HTTP 401
- **FR-006**: The system MUST send an email notification to a configured administrator address when the portal has been unreachable for a configurable number of consecutive check cycles
- **FR-007**: The system MUST display a browseable, publicly accessible list of construction projects ordered by most recent meeting date by default
- **FR-008**: Users MUST be able to view full details of any construction project, including the council decision, meeting reference, and project location
- **FR-009**: The system MUST present all user-facing interface elements in both English and French, switching based on the user's language preference
- **FR-010**: Users MUST be able to filter the project list by borough/arrondissement, by date range, and by free-text search against address and project description
- **FR-011**: The system MUST display a chronological timeline of decisions for projects that appear across multiple meetings
- **FR-012**: The system MUST link each tracked project back to the source meeting document URL for reference and verification

### Key Entities

- **Meeting**: A single City Council session, identified by date and reference number, with the source document URL and import status
- **AgendaItem**: A single agenda item within a meeting that relates to construction or development, containing the raw description and extracted structured fields
- **ConstructionProject**: A tracked construction or development project with normalized address, project type, borough, and current status; linked to one or more agenda items across meetings. Identity is determined by city dossier/permit reference number when present; otherwise by normalized address
- **ProjectDecision**: A council decision for a project at a specific meeting, recording the decision type (approved, deferred, rejected, amended) and any attached conditions

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: New meetings published on the Montreal portal appear in the site within 24 hours of publication, without any manual action
- **SC-002**: 100% of discovered meeting documents result in at least one extracted construction item or a logged "no construction items found" report — zero silent failures
- **SC-003**: Users can navigate from the project list to a project's full details in 2 clicks or fewer
- **SC-004**: The project list loads within 2 seconds for any user regardless of the total number of imported meetings
- **SC-005**: Re-processing an already-imported meeting produces zero duplicate project records
- **SC-006**: All user-facing text, labels, and navigation are fully available in both English and French
- **SC-007**: Users can locate a construction project by address or borough without prior knowledge of meeting dates or reference numbers

## Assumptions

- Meeting documents are published as PDF files linked from a publicly accessible HTML index page on the Montreal City Council portal; no authentication is required to access either the index or the PDFs
- The primary language of meeting documents is French; translation of raw project descriptions to English is out of scope for v1 — structured fields (decision type, project category) will be bilingual, but raw agenda text remains in French
- Automatic discovery polls the portal on a regular schedule (exact frequency to be determined at planning); real-time push notification from the city is not expected to be available
- The feature covers Montreal City Council (conseil municipal) meetings, not individual borough (arrondissement) council meetings, for the initial release
- "Construction projects" include any agenda item identified by section heading (e.g., "Urbanisme", "Permis et dérogations") or French construction/development keywords (e.g., *permis de construction*, *dérogation*, *zonage*, *démolition*); the keyword and section lists are configurable without a code change
- Users accessing the public project list require no login; the admin section (import log, freshness dashboard) requires authentication
- Admin credentials are managed as server environment variables: ADMIN_USER (username) and ADMIN_PASSWORD_HASH (bcrypt hash of password); HTTP Basic Auth is used for the admin section
- A public API for third-party data consumption is out of scope for v1
- An outbound email capability is available to the server for delivering administrator alerts; email configuration (SMTP credentials, recipient address) is a deployment-time concern
