# ADR 013 — Infrastructure Land Acquisition as a Second Qualification Path

**Status**: Accepted
**Date**: 2026-07-26
**Feature**: Design: Infrastructure Land-Acquisition Qualification Path
(`docs/superpowers/specs/2026-07-26-infrastructure-land-acquisition-qualification-design.md`)

## Context

The extraction pipeline recognized exactly one qualifying category of council item:
RULE-001 physical work (`extractor::validator::validate_physical_work`), gated further by
the presence of a building-scale indicator (`extractor::scale::has_scale_indicator` —
units, GFA, or storeys). Anything else made `extract_entities` return `Ok(None)`, so no
`project_mentions` row, no resolved project, and no link back to the source document was
ever created.

A real item from the January 26, 2026 Montreal city council procès-verbal (`CM26 0091`,
already a fixture in `apps/web/pipeline/tests/pipeline_extraction_fr.rs`) exposed the gap:
the city approves an addendum to a purchase promise to *acquire land* "pour les fins de
réaménagement d'infrastructures routières" at a named Lachine intersection. That is a
real, specific, future construction project residents and journalists would want tracked,
but it failed today's gate twice over — no `PHYSICAL_WORK_KEYWORDS_FR` match (only
"réaménagement," deliberately not in that list), and its only quantity is the parcel's
area (223 m²), not a building's GFA/units/storeys.

Widening RULE-001 or the scale rule to admit this item was not acceptable: both are
deliberately strict, and both already correctly exclude a large volume of administrative
council business (rezoning-only motions, budget votes, land *sales* for housing) that the
existing fixture set locks in.

Options considered:

| Option | Description |
|--------|-------------|
| Second, narrower qualification path gated on **both** land-acquisition keywords **and** an LLM-reported `project_type` of `infrastructure` | Keeps RULE-001 untouched; requires two independent signals to agree, so neither keyword noise nor a stray `project_type` alone can admit an item |
| Keyword-only gate (land-acquisition phrases, no `project_type` requirement) | Simpler, but sweeps in land transactions of every kind — notably the `CM26 0046` housing land-sale fixture that must stay excluded |
| `project_type`-only gate (any `infrastructure`-tagged item, no keyword requirement) | Admits every infrastructure-flavoured administrative item (reports received for information, funding bylaws) with no described project at all |
| Loosen RULE-001 / `PHYSICAL_WORK_KEYWORDS_*` to include "réaménagement", or drop the scale gate globally | Changes the meaning of `physical_work` and weakens a filter whose strictness is load-bearing across all existing fixtures |
| Reuse the existing `physical_work` boolean to mark these mentions as qualifying | No migration needed, but makes `physical_work` mean two different things and silently corrupts every downstream reader of that column |
| Add a new persisted `qualification_path` column recording *why* each mention qualified | One extra column; makes the classification explicit, auditable, and queryable per mention |
| Also add intersection-aware address normalization and resolver changes | Would improve later linkage of the follow-on construction item, but is a separate, much larger change than this problem requires |

## Decision

Add a second qualification path — infrastructure land acquisition — alongside RULE-001,
and record which path each mention qualified through.

- **Hybrid keyword + `project_type` gate.** New
  `validator::is_infrastructure_land_acquisition(chunk_text, language, project_type)`
  returns true only when *both* a language-specific land-acquisition keyword matches the
  chunk text *and* the LLM-reported `project_type` is infrastructure. Neither signal alone
  qualifies. The `project_type` check is a case-insensitive **prefix** match on
  `"infrastructure"`, not an exact equality test, because `project_type` is documented free
  text and the FR prompt's own worked examples return French values — real returns include
  "infrastructures", "infrastructure routière", and "infrastructures routières". Chunk text
  is lowercased and U+2019 (’) is folded to ASCII `'` before matching, since real Montreal
  PDFs use the typographic apostrophe.
- **Scale-gate exemption.** `scale::has_scale_indicator` is applied only on the
  physical-work path. Items on the new path are pre-construction by definition — there is
  no building GFA/units/storeys to report yet — so requiring one would make the path
  unreachable. Nothing synthesizes a placeholder scale value; the three scale columns
  stay `NULL`.
- **New `project_mentions.qualification_path` column, not a reuse of `physical_work`**
  (migration `028_project_mentions_qualification_path.sql`): `TEXT NOT NULL DEFAULT
  'physical_work' CHECK (qualification_path IN ('physical_work',
  'infrastructure_land_acquisition'))`, mirrored in code by
  `schema::QualificationPath`. `physical_work: bool` keeps its exact existing meaning
  (RULE-001's verdict, true only for described physical construction) and is written
  `false` for mentions on the new path.
- **Prompt clarification, not prompt rewrite.** Both `prompts::en` and `prompts::fr` gain
  one additive clause to their `has_mention` instruction stating that a land acquisition
  made specifically to enable a described future infrastructure project counts as
  describing a project. Without it the LLM's `has_mention: false` short-circuits
  `extract_entities` before any of this path's logic runs.
- **Resolver and address normalization are explicitly deferred.** No intersection-aware
  address parsing, no new cross-reference logic.

## Rationale

- Requiring two independent signals to agree is what keeps this path narrow enough to add
  without re-litigating RULE-001. The two existing real non-qualifying fixtures
  (`CM26 0046` housing land sale, `CM26 0082` architecture contract) still fail it for
  their original reasons, and no synthetic rezoning-only or administrative fixture is
  affected — none of them produce an infrastructure `project_type`.
- The scale gate exists to filter vague, non-specific mentions of construction. On a
  land-acquisition item the specificity comes from the named location and the named
  purpose instead, so the gate's rationale simply does not apply there; keeping it would
  make the path dead code.
- A separate column rather than an overloaded boolean means the classification is
  recoverable after the fact: operators and analytics can distinguish "this project is
  tracked because someone is building something" from "this project is tracked because the
  city bought land for it," which are materially different confidence levels for a reader.
  Overloading `physical_work` would have made that distinction unrecoverable and broken
  every existing consumer's assumption about the column.
- The `DEFAULT 'physical_work'` backfills correctly by construction — every row that
  existed before this change qualified via RULE-001 — so no backfill migration or script
  is needed.
- Deferring the resolver/address work keeps this change proportionate to the problem
  (an item that produced *no record at all* now produces one). Address matching for the
  eventual follow-on construction item is a pre-existing resolver limitation, unchanged by
  this decision and better addressed on its own evidence.

## Consequences

- **A council item can now become a tracked project with no scale data whatsoever.**
  Downstream views must tolerate a mention (and resolved project) whose
  `scale_units`/`scale_gfa_sqm`/`scale_storeys` are all `NULL`. Production
  field-completeness scoring (`pipeline::metrics::average_completeness`) still counts
  scale presence in its denominator for every mention, so these mentions depress the
  reported completeness figure even when extraction worked exactly as designed. This is a
  known, accepted reporting wrinkle, not a data defect; the FR integration test's own
  scoring was corrected for it, production metrics were deliberately left alone as a
  broader, system-wide change.
- **The path's reach depends on the LLM's free-text `project_type`.** A prefix match on
  "infrastructure" covers the plausible French and English variants, but an item the model
  tags "voirie", "routier", or "transport" will not qualify. This is a deliberate
  precision-over-recall trade: a wider `project_type` acceptance would have to be paid for
  with a correspondingly narrower keyword list.
- **A later item describing the actual road construction may create a separate project.**
  `resolve_mention` matches on `(civic_address_normalized, project_type)`; an intersection
  phrased differently in a later document will not match the address created here. This
  is the pre-existing resolver limitation the decision explicitly declines to fix.
- **Adding a third qualification path is now a schema change**, since the column carries a
  `CHECK` constraint enumerating the valid values. That is intentional — the set of
  reasons an item can be tracked should not grow silently.
