# Infrastructure Land-Acquisition Qualification Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let council items describing a land purchase made specifically to enable a
known future infrastructure project (e.g. a road reconfiguration at a named
intersection) qualify for a `project_mentions` row — and therefore a linked project and
source document — without loosening RULE-001's physical-work gate itself.

**Architecture:** `extractor::extract_entities` gains a second qualification path
alongside RULE-001 (`validator::validate_physical_work`): a new
`validator::is_infrastructure_land_acquisition` check requiring both an FR/EN
land-acquisition keyword match and an LLM-reported `project_type` of `"infrastructure"`.
This path is exempt from the scale-indicator gate (it has no building GFA/units/storeys
to report). Which path a mention qualified through is recorded on a new
`project_mentions.qualification_path` column so `physical_work` keeps its exact,
unmodified meaning.

**Tech Stack:** Rust, sqlx (Postgres, offline query cache at `apps/web/.sqlx`), Axum
workspace (`domain`/`pipeline`/`web` crates), `cargo nextest`.

## Global Constraints

- `physical_work: bool` must continue to mean exactly what it means today (RULE-001
  physical-work classification) — never repurposed or set `true` for this new category.
- No intersection-aware address parsing/normalization — out of scope (spec Non-goals).
- No resolver (`resolver/mod.rs`) changes — existing `(civic_address_normalized,
  project_type)` matching is reused as-is (spec Non-goals).
- No UI/template changes — `project_type = "infrastructure"` is already a recognized
  category end-to-end (spec Non-goals).
- New DB column must backfill existing rows correctly via a `DEFAULT`, not a separate
  backfill script (spec: migration `028_project_mentions_qualification_path.sql`).
- Requiring **both** a keyword match and `project_type = "infrastructure"` (case-folded)
  is the qualification test — neither signal alone is sufficient (spec Design §1).

---

### Task 1: Migration — `project_mentions.qualification_path` column

**Files:**
- Create: `apps/web/web/migrations/028_project_mentions_qualification_path.sql`
- Test: `apps/web/pipeline/src/normalizer/mod.rs` (existing `#[sqlx::test]` suite — run
  as a smoke check that the migration applies cleanly; no new test file needed for a
  pure schema change)

**Interfaces:**
- Produces: a `qualification_path TEXT NOT NULL` column on `project_mentions`,
  constrained to `'physical_work'` or `'infrastructure_land_acquisition'`, defaulting to
  `'physical_work'`.

- [ ] **Step 1: Write the migration**

```sql
-- apps/web/web/migrations/028_project_mentions_qualification_path.sql
ALTER TABLE project_mentions
    ADD COLUMN qualification_path TEXT NOT NULL DEFAULT 'physical_work'
    CHECK (qualification_path IN ('physical_work', 'infrastructure_land_acquisition'));
```

- [ ] **Step 2: Apply the migration against the dev database and confirm it runs clean**

Run (from `apps/web/`, with the dev Postgres container up — `docker compose up -d
postgres` if it isn't already):

```bash
cd apps/web && sqlx migrate run --source web/migrations
```

Expected: output ends with `Applied 28/migrate project_mentions_qualification_path
(<time>)` and no error.

- [ ] **Step 3: Run the existing pipeline test suite to confirm nothing regressed**

```bash
cd apps/web && cargo nextest run -p shovelsup-pipeline
```

Expected: all tests pass (this task adds no new tests of its own — it's a pure schema
addition exercised by later tasks).

- [ ] **Step 4: Commit**

```bash
git add apps/web/web/migrations/028_project_mentions_qualification_path.sql
git commit -m "feat(pipeline): add project_mentions.qualification_path column"
```

---

### Task 2: `QualificationPath` enum

**Files:**
- Modify: `apps/web/pipeline/src/extractor/schema.rs`

**Interfaces:**
- Consumes: nothing new (existing `RawExtraction`/`ExtractionResult` structs already in
  this file).
- Produces: `pub enum QualificationPath { PhysicalWork, InfrastructureLandAcquisition }`
  with `pub fn as_str(self) -> &'static str` returning `"physical_work"` /
  `"infrastructure_land_acquisition"` (matching Task 1's `CHECK` values exactly).
  `ExtractionResult` gains `pub qualification_path: QualificationPath` as a new field
  (added after `physical_work`, before `project_name`, to match the struct's existing
  field order convention of "classification flags first").

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of
`apps/web/pipeline/src/extractor/schema.rs`:

```rust
    #[test]
    fn qualification_path_as_str_matches_migration_check_constraint_values() {
        assert_eq!(QualificationPath::PhysicalWork.as_str(), "physical_work");
        assert_eq!(
            QualificationPath::InfrastructureLandAcquisition.as_str(),
            "infrastructure_land_acquisition"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails to compile**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::schema::tests::qualification_path
```

Expected: FAIL to compile — `QualificationPath` is not defined.

- [ ] **Step 3: Add the enum and the new `ExtractionResult` field**

In `apps/web/pipeline/src/extractor/schema.rs`, above the `ExtractionResult` struct
definition, add:

```rust
/// Which gate a mention qualified through — `physical_work` is RULE-001's
/// ordinary physical-construction path; `infrastructure_land_acquisition`
/// is the narrower land-purchase-for-future-infrastructure path (see
/// `validator::is_infrastructure_land_acquisition`). Persisted verbatim as
/// `project_mentions.qualification_path` (migration
/// `028_project_mentions_qualification_path.sql`), so `as_str()`'s values
/// must match that column's `CHECK` constraint exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualificationPath {
    PhysicalWork,
    InfrastructureLandAcquisition,
}

impl QualificationPath {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PhysicalWork => "physical_work",
            Self::InfrastructureLandAcquisition => "infrastructure_land_acquisition",
        }
    }
}
```

Then add the field to `ExtractionResult`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionResult {
    pub physical_work: bool,
    pub qualification_path: QualificationPath,
    pub project_name: Option<String>,
    pub civic_address: Option<String>,
    pub project_type: Option<String>,
    pub scale_units: Option<i32>,
    pub scale_gfa_sqm: Option<f64>,
    pub scale_storeys: Option<i32>,
    pub approval_status_raw: Option<String>,
    pub reference_number: Option<String>,
}
```

Adding a required field to `ExtractionResult` breaks compilation everywhere that struct
is constructed. It's constructed in exactly one place in non-test code —
`extractor/mod.rs`'s `extract_entities` — so fix that construction site now with the
correct value (every mention reaching that line today qualified via RULE-001, so
`QualificationPath::PhysicalWork` is accurate here; Task 4 replaces this hardcoded value
with a properly computed one). In `apps/web/pipeline/src/extractor/mod.rs`:

1. Add `QualificationPath` to the existing schema import:

```rust
use schema::{ExtractionResult, QualificationPath, RawExtraction};
```

2. In the `Ok(Some(ExtractionResult { ... }))` literal at the end of `extract_entities`
   (currently ending around line 100), add the field:

```rust
    Ok(Some(ExtractionResult {
        physical_work,
        qualification_path: QualificationPath::PhysicalWork,
        project_name,
        civic_address: raw.civic_address,
        project_type: raw.project_type,
        scale_units: raw.scale_units,
        scale_gfa_sqm: raw.scale_gfa_sqm,
        scale_storeys: raw.scale_storeys,
        approval_status_raw,
        reference_number: raw.reference_number,
    }))
```

- [ ] **Step 4: Run test to verify it passes, and confirm the crate still builds**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::schema::tests::qualification_path_as_str_matches_migration_check_constraint_values
cd apps/web && cargo nextest run -p shovelsup-pipeline
```

Expected: the new test PASSES, and the full pipeline suite still PASSES with no
regressions — this task leaves the crate in a fully green state, not a broken
intermediate one.

- [ ] **Step 5: Commit**

```bash
git add apps/web/pipeline/src/extractor/schema.rs
git commit -m "feat(pipeline): add QualificationPath enum and ExtractionResult field"
```

---

### Task 3: Infrastructure land-acquisition keyword gate

**Files:**
- Modify: `apps/web/pipeline/src/extractor/validator.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn is_infrastructure_land_acquisition(chunk_text: &str, language: &str,
  project_type: Option<&str>) -> bool` — used by Task 4.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of
`apps/web/pipeline/src/extractor/validator.rs`:

```rust
    #[test]
    fn infrastructure_land_acquisition_matches_french_keyword_and_type() {
        let text = "Approuver le projet d'addenda ... la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame.";
        assert!(is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("infrastructure")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_matches_english_keyword_and_type() {
        let text = "The City will acquire the land for infrastructure reconfiguration at the corner of Elm and Main.";
        assert!(is_infrastructure_land_acquisition(
            text,
            "en",
            Some("infrastructure")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_rejects_keyword_with_wrong_project_type() {
        let text = "La Ville vend un immeuble à des fins d'habitation, à la Coopérative d'habitation Monde-Uni.";
        // "vend" isn't a land-acquisition keyword either, but even paired
        // with an infrastructure type this housing-sale text has no
        // land-acquisition keyword match at all.
        assert!(!is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("infrastructure")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_rejects_keyword_with_non_infrastructure_type() {
        let text = "La Ville s'est engagée à acquérir un terrain à des fins d'habitation.";
        assert!(!is_infrastructure_land_acquisition(
            text,
            "fr",
            Some("residential")
        ));
    }

    #[test]
    fn infrastructure_land_acquisition_rejects_when_project_type_is_none() {
        let text = "La Ville s'est engagée à acquérir un terrain pour les fins de réaménagement d'infrastructures routières.";
        assert!(!is_infrastructure_land_acquisition(text, "fr", None));
    }

    #[test]
    fn infrastructure_land_acquisition_type_match_is_case_insensitive() {
        let text = "The City will acquire the land for infrastructure reconfiguration.";
        assert!(is_infrastructure_land_acquisition(
            text,
            "en",
            Some("Infrastructure")
        ));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::validator::tests::infrastructure_land_acquisition
```

Expected: FAIL to compile — `is_infrastructure_land_acquisition` is not defined.

- [ ] **Step 3: Add the keyword lists and the function**

In `apps/web/pipeline/src/extractor/validator.rs`, after the existing
`PHYSICAL_WORK_KEYWORDS_FR` const block, add:

```rust
/// Keywords for the narrower infrastructure-land-acquisition qualification
/// path (see `is_infrastructure_land_acquisition`): a land purchase the
/// city makes specifically to enable a known future infrastructure
/// project (e.g. a road reconfiguration), which has no described physical
/// construction yet and so never matches `PHYSICAL_WORK_KEYWORDS_*` above.
const INFRASTRUCTURE_LAND_KEYWORDS_EN: &[&str] = &[
    "land acquisition",
    "acquire the land",
    "acquire a parcel",
    "purchase of land for",
    "infrastructure reconfiguration",
    "road reconfiguration",
];

const INFRASTRUCTURE_LAND_KEYWORDS_FR: &[&str] = &[
    "acquérir un terrain",
    "acquisition d'un terrain",
    "réaménagement d'infrastructures",
    "réaménagement d'infrastructures routières",
    "aux fins de réaménagement",
];

/// Narrower, separate qualification path from `validate_physical_work`:
/// requires BOTH a land-acquisition keyword match AND an LLM-reported
/// `project_type` of `"infrastructure"` (case-insensitive). Neither signal
/// alone is sufficient — an infrastructure-tagged item with no
/// land-acquisition language, or land-acquisition language on a
/// non-infrastructure item (e.g. a housing land sale), does not qualify
/// through this path.
pub fn is_infrastructure_land_acquisition(
    chunk_text: &str,
    language: &str,
    project_type: Option<&str>,
) -> bool {
    let is_infrastructure_type = project_type
        .map(|t| t.eq_ignore_ascii_case("infrastructure"))
        .unwrap_or(false);
    if !is_infrastructure_type {
        return false;
    }
    let lower = chunk_text.to_lowercase();
    let keywords = match language {
        "fr" => INFRASTRUCTURE_LAND_KEYWORDS_FR,
        _ => INFRASTRUCTURE_LAND_KEYWORDS_EN,
    };
    keywords.iter().any(|kw| lower.contains(kw))
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::validator::tests::infrastructure_land_acquisition
```

Expected: all 6 new tests PASS. Also run the full validator test module to confirm no
regression of existing tests:

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::validator::tests
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/pipeline/src/extractor/validator.rs
git commit -m "feat(pipeline): add infrastructure land-acquisition qualification gate"
```

---

### Task 4: Wire the new path into `extract_entities`

**Files:**
- Modify: `apps/web/pipeline/src/extractor/mod.rs:36-101` (the `extract_entities`
  function body) and every `ExtractionResult { ... }` literal in its `#[cfg(test)]`
  block that currently omits `qualification_path`

**Interfaces:**
- Consumes: `QualificationPath` (Task 2), `validator::is_infrastructure_land_acquisition`
  (Task 3).
- Produces: `extract_entities` now returns `Some(ExtractionResult)` for
  infrastructure-land-acquisition text even when `physical_work` is `false`, with
  `qualification_path == QualificationPath::InfrastructureLandAcquisition` and all three
  scale fields `None`; unchanged behavior for the existing physical-work path.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `apps/web/pipeline/src/extractor/mod.rs`
(near the other `extract_entities_*` tests):

```rust
    /// A land purchase for a future road reconfiguration — no described
    /// physical construction and no building-scale indicator — still
    /// qualifies via the infrastructure-land-acquisition path when the LLM
    /// tags `project_type: "infrastructure"`.
    #[tokio::test]
    async fn extract_entities_qualifies_infrastructure_land_acquisition_without_scale() {
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":false,"project_name":null,"civic_address":"intersection de l'avenue Saint-Pierre et de la rue Notre-Dame","project_type":"infrastructure","scale_units":null,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":"Adopté à l'unanimité."}"#,
        );
        let result = extract_entities(
            "CM26 0091 — Approuver le projet d'addenda ... la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame. Adopté à l'unanimité.",
            "fr",
            &llm,
        )
        .await
        .unwrap();
        let extraction = result.expect("expected a qualifying extraction");
        assert_eq!(
            extraction.qualification_path,
            QualificationPath::InfrastructureLandAcquisition
        );
        assert!(!extraction.physical_work);
        assert_eq!(extraction.scale_units, None);
        assert_eq!(extraction.scale_gfa_sqm, None);
        assert_eq!(extraction.scale_storeys, None);
    }

    /// An infrastructure-tagged item with no land-acquisition keyword, and
    /// no scale indicator, still qualifies neither path.
    #[tokio::test]
    async fn extract_entities_rejects_infrastructure_type_without_keyword_or_scale() {
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":false,"project_name":null,"civic_address":"100 Main St","project_type":"infrastructure","scale_units":null,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":null}"#,
        );
        let result = extract_entities(
            "Item 12: The infrastructure committee received a quarterly report for information.",
            "en",
            &llm,
        )
        .await
        .unwrap();
        assert!(result.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::tests::extract_entities_qualifies_infrastructure_land_acquisition_without_scale extractor::tests::extract_entities_rejects_infrastructure_type_without_keyword_or_scale
```

Expected: FAIL — first test fails because `qualification_path` isn't set/doesn't exist
on the constructed result yet (crate won't compile, since `ExtractionResult` literals in
this file are missing the field added in Task 2); second currently fails differently
(would incorrectly return `Ok(None)`... but compile error takes precedence). Either way:
FAIL is expected here.

- [ ] **Step 3: Update `extract_entities`'s gating logic and every test fixture's**
      **`ExtractionResult` construction**

Replace the gating block in `extract_entities` (currently `mod.rs:57-64`):

```rust
    let physical_work = validator::validate_physical_work(chunk_text, language, raw.physical_work);
    if !physical_work {
        return Ok(None);
    }

    if !scale::has_scale_indicator(raw.scale_units, raw.scale_gfa_sqm, raw.scale_storeys) {
        return Ok(None);
    }
```

with:

```rust
    let physical_work = validator::validate_physical_work(chunk_text, language, raw.physical_work);
    let qualification_path = if physical_work {
        Some(QualificationPath::PhysicalWork)
    } else if validator::is_infrastructure_land_acquisition(
        chunk_text,
        language,
        raw.project_type.as_deref(),
    ) {
        Some(QualificationPath::InfrastructureLandAcquisition)
    } else {
        None
    };
    let Some(qualification_path) = qualification_path else {
        return Ok(None);
    };

    // Scale gate applies only to the physical-work path — infrastructure
    // land-acquisition items are pre-construction and have no building
    // GFA/units/storeys to report yet.
    if qualification_path == QualificationPath::PhysicalWork
        && !scale::has_scale_indicator(raw.scale_units, raw.scale_gfa_sqm, raw.scale_storeys)
    {
        return Ok(None);
    }
```

Add the import at the top of the file (next to the existing `use schema::{...}`):

```rust
use schema::{ExtractionResult, QualificationPath, RawExtraction};
```

Update the `Ok(Some(ExtractionResult { ... }))` construction at the end of
`extract_entities` — this replaces the hardcoded `qualification_path:
QualificationPath::PhysicalWork` that Task 2 put here as a compile-fix with the actual
computed `qualification_path` variable from Step 3 above:

```rust
    Ok(Some(ExtractionResult {
        physical_work,
        qualification_path,
        project_name,
        civic_address: raw.civic_address,
        project_type: raw.project_type,
        scale_units: raw.scale_units,
        scale_gfa_sqm: raw.scale_gfa_sqm,
        scale_storeys: raw.scale_storeys,
        approval_status_raw,
        reference_number: raw.reference_number,
    }))
```

This file's `ExtractionResult` is only ever constructed in this one place in
non-test code, so no other production call site needs updating.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::tests::extract_entities_qualifies_infrastructure_land_acquisition_without_scale extractor::tests::extract_entities_rejects_infrastructure_type_without_keyword_or_scale
```

Expected: both PASS. Then run the full extractor test module to confirm no regressions:

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::tests
```

Expected: all PASS (existing tests are unaffected — they all supply `physical_work:
true` with a scale indicator, taking the unchanged `PhysicalWork` branch).

- [ ] **Step 5: Commit**

```bash
git add apps/web/pipeline/src/extractor/mod.rs
git commit -m "feat(pipeline): qualify infrastructure land-acquisition mentions without a scale indicator"
```

---

### Task 5: Persist `qualification_path` in `extract_and_store`

**Files:**
- Modify: `apps/web/pipeline/src/extractor/mod.rs:160-176` (the INSERT inside
  `extract_and_store`)

**Interfaces:**
- Consumes: `ExtractionResult.qualification_path` (Task 2/4).
- Produces: `project_mentions.qualification_path` is populated on every insert,
  readable by later tasks/callers via a plain `SELECT`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `apps/web/pipeline/src/extractor/mod.rs`,
near the other `extract_and_store_*` tests:

```rust
    #[sqlx::test(migrations = "../web/migrations")]
    async fn extract_and_store_persists_infrastructure_land_acquisition_qualification_path(
        pool: PgPool,
    ) {
        let chunk_id = seed_chunk(&pool).await;
        let llm = FixedResponseProvider::new(
            r#"{"has_mention":true,"physical_work":false,"project_name":null,"civic_address":"intersection de l'avenue Saint-Pierre et de la rue Notre-Dame","project_type":"infrastructure","scale_units":null,"scale_gfa_sqm":null,"scale_storeys":null,"approval_status_raw":"Adopté à l'unanimité."}"#,
        );

        let mention_id = extract_and_store(
            &pool,
            chunk_id,
            "... la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame.",
            &llm,
        )
        .await
        .unwrap()
        .expect("expected a qualifying mention");

        let (qualification_path, physical_work): (String, bool) = sqlx::query_as(
            "SELECT qualification_path, physical_work FROM project_mentions WHERE id = $1",
        )
        .bind(mention_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(qualification_path, "infrastructure_land_acquisition");
        assert!(!physical_work);
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::tests::extract_and_store_persists_infrastructure_land_acquisition_qualification_path
```

Expected: FAIL — `qualification_path` column exists (Task 1) but is never written, so it
stays at its `DEFAULT 'physical_work'`, not `'infrastructure_land_acquisition'`.

- [ ] **Step 3: Update the INSERT**

Replace the INSERT in `extract_and_store` (currently `mod.rs:163-176`):

```rust
            let mention_id = sqlx::query_scalar!(
                "INSERT INTO project_mentions \
                 (document_chunk_id, physical_work, project_name, civic_address, project_type, \
                  scale_units, scale_gfa_sqm, scale_storeys, approval_status_raw, reference_number) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
                document_chunk_id,
                extraction.physical_work,
                extraction.project_name,
                extraction.civic_address,
                extraction.project_type,
                extraction.scale_units,
                extraction.scale_gfa_sqm,
                extraction.scale_storeys,
                extraction.approval_status_raw,
                extraction.reference_number,
            )
            .fetch_one(pool)
            .await?;
```

with:

```rust
            let mention_id = sqlx::query_scalar!(
                "INSERT INTO project_mentions \
                 (document_chunk_id, physical_work, qualification_path, project_name, civic_address, \
                  project_type, scale_units, scale_gfa_sqm, scale_storeys, approval_status_raw, reference_number) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
                document_chunk_id,
                extraction.physical_work,
                extraction.qualification_path.as_str(),
                extraction.project_name,
                extraction.civic_address,
                extraction.project_type,
                extraction.scale_units,
                extraction.scale_gfa_sqm,
                extraction.scale_storeys,
                extraction.approval_status_raw,
                extraction.reference_number,
            )
            .fetch_one(pool)
            .await?;
```

Because this project uses sqlx's offline query cache, regenerate it against the running
dev database before the crate will build in offline mode:

```bash
cd apps/web && DATABASE_URL=postgres://shovelsup:shovelsup@localhost:5434/shovelsup cargo sqlx prepare --workspace -- --all-targets
```

(Adjust the `DATABASE_URL` port/credentials if your local `docker-compose.yml`
overrides differ from the defaults shown in Task 1's docker-compose excerpt.)

- [ ] **Step 4: Run test to verify it passes**

```bash
cd apps/web && cargo test -p shovelsup-pipeline --lib extractor::tests::extract_and_store_persists_infrastructure_land_acquisition_qualification_path
```

Expected: PASS. Then run the full pipeline suite:

```bash
cd apps/web && cargo nextest run -p shovelsup-pipeline
```

Expected: all PASS, including the pre-existing `extract_and_store_*` tests (they all
qualify via the unchanged `PhysicalWork` path, so their rows now correctly show
`qualification_path = 'physical_work'`).

- [ ] **Step 5: Commit**

```bash
git add apps/web/pipeline/src/extractor/mod.rs apps/web/.sqlx
git commit -m "feat(pipeline): persist qualification_path on every extracted mention"
```

---

### Task 6: Flip the real `CM26 0091` fixture to qualifying

**Files:**
- Modify: `apps/web/pipeline/tests/pipeline_extraction_fr.rs:66-97`

**Interfaces:**
- Consumes: nothing new (this is a fixture/doc-comment update only; the pipeline code
  under test is already correct after Task 4).
- Produces: nothing consumed by later tasks — this is the final task.

- [ ] **Step 1: Update the fixture's `should_qualify` and the preceding doc comment**

In `apps/web/pipeline/tests/pipeline_extraction_fr.rs`, the comment block currently
reads (lines 66-79):

```rust
// --- REAL FIXTURES (IMP-REQ-007-06): sourced from the genuine
// procès-verbal of the Montreal city council's January 26, 2026
// ordinary meeting (ville.montreal.qc.ca/documents/Adi_Public/CM/
// CM_PV_ORDI_2026-01-26_13h00_FR.pdf), retrieved 2026-07-11. All three
// are non-qualifying, and for different real reasons — city-level
// council items here skew toward land transactions and financing
// rather than granular building-permit decisions (that detail is
// handled at the arrondissement/borough level, a separate system not
// reachable in this session): a land sale enabling future housing
// construction (administrative, no physical work described, mirrors
// the rezoning-only exclusion), a real construction item that
// genuinely has no scale indicator in the visible resolution text
// (fails the scale gate despite being real physical work), and a land
// purchase for a future road reconfiguration (administrative).
```

Replace it with:

```rust
// --- REAL FIXTURES (IMP-REQ-007-06): sourced from the genuine
// procès-verbal of the Montreal city council's January 26, 2026
// ordinary meeting (ville.montreal.qc.ca/documents/Adi_Public/CM/
// CM_PV_ORDI_2026-01-26_13h00_FR.pdf), retrieved 2026-07-11. City-level
// council items here skew toward land transactions and financing
// rather than granular building-permit decisions (that detail is
// handled at the arrondissement/borough level, a separate system not
// reachable in this session). Two of the three are non-qualifying, for
// different real reasons: a land sale enabling future housing
// construction (administrative, no physical work described, mirrors
// the rezoning-only exclusion), and a real construction item that
// genuinely has no scale indicator in the visible resolution text
// (fails the scale gate despite being real physical work). The third —
// a land purchase for a future road reconfiguration at a named
// intersection — now qualifies via the infrastructure-land-acquisition
// path (see `extractor::validator::is_infrastructure_land_acquisition`):
// it has no building-scale indicator either, but is exempt from that
// gate on this path.
```

Then update the `CM26 0091` fixture itself (currently lines 92-97):

```rust
    Fixture {
        text: "CM26 0091 — Approuver le projet d'addenda entre la Ville de Montréal et 9519-5228 Québec inc. modifiant la promesse bilatérale d'achat et de vente par laquelle la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame, dans l'arrondissement de Lachine, d'une superficie totale de 223 mètres carrés. Adopté à l'unanimité.",
        should_qualify: false,
        has_name: false,
        has_status: true,
    },
```

to:

```rust
    Fixture {
        text: "CM26 0091 — Approuver le projet d'addenda entre la Ville de Montréal et 9519-5228 Québec inc. modifiant la promesse bilatérale d'achat et de vente par laquelle la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame, dans l'arrondissement de Lachine, d'une superficie totale de 223 mètres carrés. Adopté à l'unanimité.",
        should_qualify: true,
        has_name: false,
        has_status: true,
    },
```

- [ ] **Step 2: Run the FR integration test**

This test requires a live `ANTHROPIC_API_KEY` and skips (doesn't fail) without one —
run it with the key set to actually exercise the live classification:

```bash
cd apps/web && ANTHROPIC_API_KEY=<your key> cargo test -p shovelsup-pipeline --test pipeline_extraction_fr -- --nocapture
```

Expected: test passes. If the live API tags this item's `project_type` as something
other than `"infrastructure"`, or doesn't produce a land-acquisition keyword match, this
test will fail here — per the spec's flagged risk, that's the signal to revisit the
keyword lists in Task 3, not a plan error to silently work around.

Without an API key set, confirm it still compiles and skips cleanly:

```bash
cd apps/web && cargo test -p shovelsup-pipeline --test pipeline_extraction_fr
```

Expected: test reports skipped/ignored (matching this test's existing no-key behavior),
not a compile error.

- [ ] **Step 3: Run the full workspace test suite**

```bash
cd apps/web && cargo nextest run --workspace
```

Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/web/pipeline/tests/pipeline_extraction_fr.rs
git commit -m "test(pipeline): flip CM26 0091 fixture to qualifying via infrastructure land-acquisition path"
```

---

## Post-implementation

This plan does not include writing ADR-013 documenting this scope decision (per this
repo's ADR requirement) — write that as a follow-up once the implementation above is
merged and confirmed working, since an ADR documents a decision that was actually
carried out, not one still in progress.
