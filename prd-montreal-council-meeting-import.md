# ShovelsUp — Montreal City Council Meeting Minute Import

## Product Overview

This feature imports and parses Montreal City Council meeting agendas and minutes from the city's public portal (Ville de Montréal), extracting construction- and development-related agenda items and making them searchable inside ShovelsUp. For the MVP, the primary focus is civic accountability: giving residents, journalists, and community organizations a clear, searchable record of what the city approves, where, and when — so decisions that affect neighborhoods are visible to the people those neighborhoods belong to.

---

**Target date:** Q3 2026 (September 30, 2026)

**Document status:** Draft

**Team members:**
| Name | Role |
|------|------|
| Alex Landovskis | Engineering |

---

## Quick Links

- **Designs:** —
- **Loom demo:** —
- **Work tracker:** —

---

## Objective

Montreal City Council meeting agendas contain permit approvals, zoning changes, and construction authorizations that directly affect residents — but this information is buried in dense French-language PDFs published sporadically on a city portal. Most residents never see it. By importing, parsing, and surfacing these decisions in ShovelsUp, we give residents, journalists, and community groups a clear, searchable record of what the city approved, in which borough, and how it connects to real construction activity. The goal is to make the link between a council vote and a construction site legible to anyone — not just industry insiders.

---

## Success Metrics

| Goal | Metric |
|------|--------|
| Reliable ingestion | ≥ 95% of published conseil municipal and comité exécutif meetings successfully imported and indexed within 24 hours of publication |
| Extraction accuracy | ≥ 90% of construction/development-related agenda items correctly classified per meeting, measured against a labeled reference dataset established during the Discovery spike |
| Civic engagement | ≥ 40% of council-sourced project records viewed by at least one user within 14 days of import (measured from launch month 2 onward) |
| Council-to-permit linkage | ≥ 50% of council-approved construction items successfully linked to a downstream permit record within 6 months of council decision (measured as a cohort after 6 months post-launch) |

---

## Assumptions

Items marked **[RISK]** are unverified hypotheses that must be validated during Discovery, with the stated mitigation in place before Build begins.

- The `ville.montreal.qc.ca` public portal URL pattern and PDF document structure are stable. **[RISK]** Portal structure changes will break import silently; mitigation: automated schema validation on every import run, alerting on structural mismatch.
- French-language text extraction and keyword classification will handle agenda item content adequately. **[RISK]** Unvalidated prior to the Discovery spike; adequacy must be confirmed before Build phase begins.
- Conseil municipal and comité exécutif meetings follow a sufficiently regular publishing cadence to support scheduled polling. **[RISK]** Agendas are sometimes posted 48–72 hours before a meeting with no guaranteed schedule; polling interval and retry logic must account for irregular availability.
- Construction-, permit-, and zoning-related agenda items can be reliably identified via keyword matching or classification rules without requiring full semantic understanding. **[RISK]** Classification accuracy is an unproven hypothesis; a labeled reference dataset and baseline measurement must be produced in Discovery before committing to the extraction accuracy metric target.

---

## Milestones

| Milestone | Target Date |
|-----------|-------------|
| Discovery — portal structure analysis, PDF parsing spike, classification baseline, data model design | 2026-07-10 |
| Build — fetch scheduler, PDF parser, agenda item extractor, project matcher, project creator, observability | 2026-08-15 |
| Internal Testing — end-to-end validation against historical and live meetings | 2026-09-12 |
| Launch — production deployment, monitoring in place | 2026-09-30 |

---

## Requirements

| Requirement | User Story | Importance | Jira Issue | Notes |
|-------------|------------|------------|------------|-------|
| Scheduled PDF fetch | *As a ShovelsUp operator, I want the system to automatically fetch newly published conseil municipal and comité exécutif meeting PDFs on a schedule so that council decisions are indexed promptly without manual intervention.* | Must | — | Configurable polling interval; idempotent — re-fetching an already-imported meeting must not create duplicate records. |
| Agenda item extraction | *As a ShovelsUp operator, I want structured agenda items (title, type, borough/district, decision outcome) extracted from raw PDF text so that construction- and development-related decisions can be stored and searched reliably.* | Must | — | French-language text; classification logic validated against labeled reference dataset from Discovery spike. |
| Project matching | *As a Montreal resident, I want agenda items that correspond to an existing permit or construction project to be linked to that project so that I can see how a council decision connects to real activity in my neighborhood.* | Must | — | Fuzzy match on address and/or description; log low-confidence matches for operator review. |
| Project creation | *As a Montreal resident, I want council-approved items with no existing permit record to appear as new project entries so that I can track development decisions that have not yet reached the permit stage.* | Must | — | New entries tagged as "council-sourced" and displayed with appropriate context (no permit yet filed). |
| Borough and date search | *As a Montreal resident or journalist, I want to search and filter council decisions by borough, date range, and decision type so that I can find what was approved in a specific area or time period.* | Must | — | Borough/district extracted at the agenda item level; date corresponds to meeting date. |
| Council-to-permit timeline view | *As a Montreal resident, I want to see the timeline from council approval to permit filing to construction start for a given project so that I can understand how long decisions take to translate into activity and hold the city accountable.* | Should | — | Requires project matching to be working; display as a simple milestone trail on the project detail page. |
| Pipeline observability | *As a ShovelsUp operator, I want visibility into import failures, parse errors, and zero-result meetings so that I can detect and remediate pipeline issues before they cause data gaps.* | Could | — | Minimum: structured error logging and an alert on repeated fetch or parse failures. |

---

## Out of Scope

- Other municipalities — only Ville de Montréal (conseil municipal and comité exécutif) for this MVP.
- Borough councils, advisory committees, and other Montreal meeting bodies — may be added in a future iteration.
- Real-time or streaming updates — scheduled batch import only; no live meeting feeds.
- Historical backfill beyond 12 months — the initial import covers the past 12 months; a full historical archive is out of scope.
- Automated alerts or notifications to residents — no push alerts triggered by new council decisions in this iteration.
- Council member voting records — individual vote attribution is out of scope for MVP.

---

## Design

The import pipeline runs entirely as an asynchronous background job; no user-facing request blocks on or is delayed by fetch or parse operations. The job scheduler is idempotent: re-running an import for a previously processed meeting must not create duplicate records.

PDF-to-text extraction must preserve the hierarchical structure of the agenda (sections, sub-items) to aid classification accuracy. The polling mechanism must account for irregular agenda publication cadence (see Assumptions) with configurable retry windows.

Council-sourced project entries must be clearly distinguished in the UI from permit-sourced entries — users should never be confused about whether a permit has been filed. A "council decision only — no permit filed yet" indicator is required on all council-sourced stubs until a permit match is established.

**Initial import window:** On first run, the pipeline imports the past 12 months of conseil municipal and comité exécutif meetings to seed the project database. Subsequent runs are incremental.

**Classification ground truth:** During Discovery, a labeled reference dataset of ≥ 5 historical meetings will be produced to establish the 90% extraction accuracy baseline and guide keyword/rule development.

---

## Open Questions

| Question | Answer | Date Answered |
|----------|--------|---------------|
| Which Montreal meeting bodies are in scope beyond conseil municipal and comité exécutif? | TBD — confirm in Discovery | — |
| Are there terms-of-use or rate-limiting constraints on the ville.montreal.qc.ca portal? | TBD — review in Discovery | — |
| Should council decisions that are rejected (not approved) also be surfaced? | TBD — civic accountability case may warrant showing rejections | — |

---

## Reference Links

- Sample meeting agenda PDF: https://ville.montreal.qc.ca/sel/adi-public/afficherpdf/fichier.pdf?typeDoc=odj&doc=17333
- Ville de Montréal public meeting portal: https://ville.montreal.qc.ca/sel/adi-public/
