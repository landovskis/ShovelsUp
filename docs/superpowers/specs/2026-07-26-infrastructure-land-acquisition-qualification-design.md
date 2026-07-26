# Design: Infrastructure Land-Acquisition Qualification Path

**Date**: 2026-07-26
**Related**: `apps/web/pipeline/src/extractor/mod.rs`, `apps/web/pipeline/src/extractor/validator.rs`,
`apps/web/pipeline/src/extractor/schema.rs`, `apps/web/pipeline/src/extractor/scale.rs`,
`apps/web/pipeline/tests/pipeline_extraction_fr.rs`

## Problem

The extraction pipeline currently recognizes exactly one qualifying category of council
item: RULE-001 physical work (`validator::validate_physical_work`), gated further by a
scale indicator (`scale::has_scale_indicator`). Everything else — including land
purchases the city makes specifically to enable a known future infrastructure project —
is discarded (`extract_entities` returns `Ok(None)`), so no `project_mentions` row is
ever created and there is nothing to link back to the source document.

This was surfaced by a real item from the January 26, 2026 Montreal city council
procès-verbal (`CM26 0091`, already present as a fixture in
`apps/web/pipeline/tests/pipeline_extraction_fr.rs:92-97`):

> Approuver le projet d'addenda entre la Ville de Montréal et 9519-5228 Québec inc.
> modifiant la promesse bilatérale d'achat et de vente par laquelle la Ville s'est
> engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures
> routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame,
> dans l'arrondissement de Lachine, d'une superficie totale de 223 mètres carrés.
> Adopté à l'unanimité.

This describes a real, specific future construction project (a road reconfiguration at
a named intersection) that residents/journalists would reasonably want to see tracked —
but it fails today's gate twice over: it contains no `PHYSICAL_WORK_KEYWORDS_FR` match
(only "réaménagement," which isn't in that list), and its only quantity is the land
parcel's area (223 m²), not a building's GFA/units/storeys.

## Goal

Add a second, narrower qualification path — infrastructure land acquisition — so items
like this one produce a `project_mentions` row (and therefore a linked project and
source document), without loosening RULE-001 itself or accidentally sweeping in
unrelated administrative items (rezoning-only motions, budget votes, land sales for
housing, etc., all already correctly excluded and covered by existing fixtures).

## Non-goals

- No intersection-aware address parsing/normalization. `civic_address` remains
  whatever the LLM extracts freeform, normalized as a single opaque string by
  `resolver/address.rs` / `address_fr.rs`, exactly as today. If a later item describing
  the actual road construction phrases the location differently than this one, it may
  fail to match on `civic_address_normalized` and create a separate project — a
  pre-existing limitation of the resolver, not something this change fixes.
- No new resolver/cross-reference logic. `resolve_mention` already matches on
  `(civic_address_normalized, project_type)`; a later scale-bearing item at the same
  address with `project_type = "infrastructure"` will attach to the project created
  here automatically, with no code change needed in `resolver/mod.rs`.
- No change to how `physical_work: bool` is computed or what it means — it continues to
  reflect RULE-001 exactly (true only for actual described physical construction). This
  category does not redefine or repurpose that column.
- No UI/template changes. `project_type = "infrastructure"` is already a recognized
  category end-to-end (search facets, `category_display_name` in
  `apps/web/web/src/routes/search.rs`), so nothing downstream needs to learn a new
  value.

## Design

### 1. New keyword lists + gate (`validator.rs`)

Add EN/FR keyword lists for infrastructure land acquisition, alongside the existing
rezoning/physical-work lists:

```rust
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
```

New function, deterministic like `validate_physical_work`, but requiring **both** a
keyword match **and** an LLM-reported `project_type` of `"infrastructure"` (case
folded) — the hybrid gate:

```rust
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

Requiring both signals keeps this narrow: neither an infrastructure-tagged item with no
land-acquisition language, nor land-acquisition language on a non-infrastructure item
(e.g. the existing `CM26 0046` housing land-sale fixture), qualifies through this path.

### 2. `QualificationPath` (`schema.rs`)

```rust
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

`ExtractionResult` gains `pub qualification_path: QualificationPath`. `physical_work:
bool` is unchanged and stays present.

### 3. Gating logic (`extractor/mod.rs`)

Current flow in `extract_entities`:

```rust
let physical_work = validator::validate_physical_work(chunk_text, language, raw.physical_work);
if !physical_work { return Ok(None); }
if !scale::has_scale_indicator(raw.scale_units, raw.scale_gfa_sqm, raw.scale_storeys) {
    return Ok(None);
}
```

New flow:

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

`ExtractionResult { physical_work, qualification_path, .. }` is constructed with both
fields; `scale_units`/`scale_gfa_sqm`/`scale_storeys` stay `None` for the new path
(nothing synthesizes a fake value for them).

### 4. Persistence

Migration `apps/web/web/migrations/028_project_mentions_qualification_path.sql`:

```sql
ALTER TABLE project_mentions
    ADD COLUMN qualification_path TEXT NOT NULL DEFAULT 'physical_work'
    CHECK (qualification_path IN ('physical_work', 'infrastructure_land_acquisition'));
```

The `DEFAULT` backfills every existing row correctly, since every row that exists today
qualified via the physical-work path — no separate backfill migration/script needed.

`extract_and_store`'s INSERT (`extractor/mod.rs`) gains the column:

```sql
INSERT INTO project_mentions
    (document_chunk_id, physical_work, qualification_path, project_name, civic_address,
     project_type, scale_units, scale_gfa_sqm, scale_storeys, approval_status_raw, reference_number)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
```

passing `extraction.qualification_path.as_str()`.

## Testing

- `validator.rs`: new unit tests for `is_infrastructure_land_acquisition` —
  - matches on keyword + `project_type = "infrastructure"` (FR and EN).
  - rejects keyword match with a non-infrastructure `project_type`.
  - rejects `project_type = "infrastructure"` with no keyword match.
  - rejects when `project_type` is `None`.
- `extractor/mod.rs`: new unit test feeding a mocked LLM response with
  `physical_work: false`, `project_type: "infrastructure"`, no scale fields, and
  land-acquisition keyword text, asserting `extract_entities` returns
  `Some(ExtractionResult)` with `qualification_path ==
  QualificationPath::InfrastructureLandAcquisition` and all three scale fields `None`.
- `pipeline_extraction_fr.rs`: flip the `CM26 0091` fixture's `should_qualify` to
  `true`; update the preceding doc comment (currently states all 3 real fixtures are
  non-qualifying) to explain this one now qualifies via the new path, while the other
  two real fixtures (`CM26 0046` housing land sale, `CM26 0082` architecture contract)
  remain non-qualifying for their existing, unrelated reasons.
- Existing fixtures/tests (`CM26 0046`, all EN/FR rezoning-only and ambiguous cases)
  are unaffected — the new gate requires `project_type = "infrastructure"`, which none
  of those produce.

## Risk

The real Anthropic API's `project_type` classification for `CM26 0091` is not
independently verified in this design — it's inferred from the prompt's own worked
example listing "infrastructure" alongside residential/commercial/institutional as a
valid category, and the source text's explicit "réaménagement d'infrastructures
routières" phrasing. The FR integration test (`pipeline_extraction_fr.rs`) that
exercises this fixture already skips (not fails) when `ANTHROPIC_API_KEY` is unset, so
if the live API tags it differently, that test will surface it as a failure to
investigate rather than a silent gap.
