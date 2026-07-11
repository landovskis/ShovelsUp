//! IMP-REQ-003-08/09: labelled fixture set + field-completeness integration
//! test (TC-REQ-003-1, and the field-completeness gate from Autonomous
//! Execution Notes: "use the ≥90% field-completeness metric as the interim
//! launch gate").
//!
//! SCOPE REDUCTION (flagged explicitly, not silently shipped): the plan
//! calls for a ≥200-item hand-labelled fixture set across 3 municipalities.
//! I cannot authentically produce that — it requires real scraped municipal
//! documents with human-verified ground truth, which I have no access to
//! and cannot fabricate without the result being fake data presented as
//! real. This is a 30-item *synthetic* set (clearly-constructed English
//! sentences in the style of council agenda items, not real documents),
//! covering physical-work vs. rezoning-only cases and single/multi scale
//! indicators. It exercises the real extraction pipeline (RULE-001, scale
//! rule, and — when ANTHROPIC_API_KEY is set — the real Anthropic API) but
//! is not a substitute for the real labelled set the plan asks for. See
//! IMPLEMENTATION_CHECKLIST.md REQ-003 risks.
//!
//! UNRESOLVED FINDING (real, reproduced against the live API, not a code
//! bug): measured field completeness against this fixture set is ~85%,
//! below the 90% interim gate — stable across two runs (84.7%, 85.0%) after
//! six rounds of real prompt-engineering iteration (see prompts::en git
//! history: worked example, effort=high, field reordering, terser vs.
//! verbose instructions were all tried). Classification accuracy
//! (has_mention/physical_work) is 97-100% — the gap is specifically
//! approval_status_raw: a short trailing decision clause ("Approved.",
//! "Deferred.") is inconsistently populated by the model even when
//! literally present and even with an explicit worked example demonstrating
//! it. This is left as an open, honestly-measured result rather than
//! weakening the assertion to force a pass — matches the plan's own Open
//! Risk ("Extraction precision/recall threshold not set by the PRD",
//! Founder, target 2026-07-20). Next steps for whoever picks this up:
//! try structured "quote the final sentence first" chain-of-thought, a
//! second-pass status-only extraction call, or revisit whether 90% is the
//! right bar before the labelled set is the real 200-item one.

use shovelsup_web::pipeline::extractor::extract_entities;
use shovelsup_web::pipeline::extractor::llm::AnthropicProvider;

struct Fixture {
    text: &'static str,
    should_qualify: bool,
    /// Ground truth: does the fixture text actually state a project name?
    /// Every qualifying fixture states an address, a type (directly or via
    /// an unambiguous use like "school"/"warehouse"), a scale indicator (by
    /// construction — that's what makes it qualify), and an approval
    /// status. Project name is the only field that's legitimately absent in
    /// some real agenda items — measuring completeness against it as if it
    /// were always present double-penalizes the model for correctly
    /// returning null on text that has no name to extract (see git history:
    /// the original 5-fields-always-required metric measured 64-80%
    /// despite the underlying extraction being accurate).
    has_name: bool,
}

const FIXTURES: &[Fixture] = &[
    // --- Qualifying: physical work with a scale indicator. Every fixture
    // states an address, type, scale, and approval status; only project
    // name legitimately varies (see `has_name` and `field_completeness`). ---
    Fixture { text: "Item 4: Application by Meridian Homes for construction of a new residential building known as \"Maple Court\" at 123 Main St, 48 units, 6 storeys. Approved.", should_qualify: true, has_name: true },
    Fixture { text: "Item 7: Demolition of the existing structure at 45 Oak Ave to permit construction of a mixed-use development known as \"Riverside Commons\", 12,000 sqm gross floor area. Deferred to next meeting.", should_qualify: true, has_name: true },
    Fixture { text: "Item 9: Renovation and addition to the institutional community centre at 200 Elm St, adding 2 storeys. Approved.", should_qualify: true, has_name: false },
    Fixture { text: "Item 11: New commercial building known as \"Pine Road Plaza\" at 78 Pine Rd, 3 storeys, 4,500 sqm. Referred to committee.", should_qualify: true, has_name: true },
    Fixture { text: "Item 15: Expansion of the existing industrial warehouse at 500 Industrial Way, adding 20 units of storage capacity and 1 storey. Approved.", should_qualify: true, has_name: false },
    Fixture { text: "Item 18: Erection of a new institutional building (library branch) known as the \"Birch Street Branch\" at 90 Birch St, 2 storeys. Approved.", should_qualify: true, has_name: true },
    Fixture { text: "Item 22: Conversion of the former industrial factory at 15 Mill St into a residential development of 60 units. Approved.", should_qualify: true, has_name: false },
    Fixture { text: "Item 25: Construction of a new mixed-use tower known as \"Bay Street Heights\" at 1000 Bay St, 24 storeys, 300 units. Deferred.", should_qualify: true, has_name: true },
    Fixture { text: "Item 29: Building permit issued for a new residential single-family dwelling at 22 Cedar Lane. Approved.", should_qualify: true, has_name: false },
    Fixture { text: "Item 33: Demolition of 3 existing units at 8 Spruce Ct to permit construction of a residential townhouse development known as \"Spruce Court Towns\", 18 units, 3 storeys. Approved.", should_qualify: true, has_name: true },
    Fixture { text: "Item 36: Addition to the existing institutional hospital at 400 Health Dr, adding 5,000 sqm of floor area. Referred to committee.", should_qualify: true, has_name: false },
    Fixture { text: "Item 40: New 10-storey commercial office building known as \"Business Parkway Tower\" at 250 Business Pkwy, 15,000 sqm GFA. Approved.", should_qualify: true, has_name: true },
    Fixture { text: "Item 44: Renovation of the existing institutional school at 60 Learning Ave, adding 8 classrooms (treated as 8 units). Approved.", should_qualify: true, has_name: false },
    Fixture { text: "Item 48: Construction of a new 4-storey infrastructure parking structure at 33 Transit Way. Approved.", should_qualify: true, has_name: false },
    Fixture { text: "Item 52: Expansion of the institutional recreation centre known as \"Sport Street Community Centre\" at 77 Sport St, adding 1 storey and a new pool wing. Approved.", should_qualify: true, has_name: true },
    // --- Non-qualifying: rezoning-only / administrative, no physical work ---
    Fixture { text: "Item 2: Zoning by-law amendment to permit mixed-use designation at 400 King St. No construction proposed at this time.", should_qualify: false, has_name: false },
    Fixture { text: "Item 5: Official plan amendment to redesignate lands at 55 River Rd from industrial to residential. Referred to committee.", should_qualify: false, has_name: false },
    Fixture { text: "Item 8: Motion to approve the annual operating budget for the planning department. Approved.", should_qualify: false, has_name: false },
    Fixture { text: "Item 13: Council received the quarterly traffic safety report for information.", should_qualify: false, has_name: false },
    Fixture { text: "Item 17: Rezoning application to change land use designation at 900 Commerce Blvd from agricultural to commercial. Deferred.", should_qualify: false, has_name: false },
    Fixture { text: "Item 20: Appointment of a new member to the heritage advisory committee. Approved.", should_qualify: false, has_name: false },
    Fixture { text: "Item 24: Council approved the minutes of the previous meeting.", should_qualify: false, has_name: false },
    Fixture { text: "Item 27: Zoning by-law amendment to update parking requirements city-wide. Approved.", should_qualify: false, has_name: false },
    Fixture { text: "Item 31: Public consultation scheduled regarding the draft transportation master plan.", should_qualify: false, has_name: false },
    Fixture { text: "Item 35: Council received a staff report on winter road maintenance for information purposes.", should_qualify: false, has_name: false },
    Fixture { text: "Item 39: Motion to award the annual snow removal contract. Approved.", should_qualify: false, has_name: false },
    Fixture { text: "Item 42: Official plan amendment to designate a new employment area at 700 Logistics Dr. Referred to committee.", should_qualify: false, has_name: false },
    Fixture { text: "Item 46: Council proclaimed the following week as Small Business Week.", should_qualify: false, has_name: false },
    Fixture { text: "Item 50: Motion to appoint an interim city clerk. Approved.", should_qualify: false, has_name: false },
    Fixture { text: "Item 54: Zoning amendment to permit a home-based business use with no described physical alterations.", should_qualify: false, has_name: false },
];

/// Completeness measured against ground truth: civic_address, project_type,
/// a scale indicator, and approval_status_raw are present in every
/// qualifying fixture's text by construction, so they're always expected.
/// project_name is only expected when `has_name` says the text actually
/// states one — a correct null on a nameless fixture must not count as
/// incomplete.
fn field_completeness(
    result: &shovelsup_web::pipeline::extractor::schema::ExtractionResult,
    fixture: &Fixture,
) -> f64 {
    let mut expected = 4;
    let mut present = [
        result.civic_address.is_some(),
        result.project_type.is_some(),
        result.scale_units.is_some() || result.scale_gfa_sqm.is_some() || result.scale_storeys.is_some(),
        result.approval_status_raw.is_some(),
    ]
    .iter()
    .filter(|f| **f)
    .count();

    if fixture.has_name {
        expected += 1;
        if result.project_name.is_some() {
            present += 1;
        }
    }

    present as f64 / expected as f64
}

/// TC-REQ-003-1 + field-completeness gate. Requires a real ANTHROPIC_API_KEY
/// — skips (not fails) when unset, so `cargo test` stays runnable without
/// network access/spend, per the same pattern used for the OCR-toolchain
/// gate in pipeline::parser::ocr::tests.
#[tokio::test]
async fn extraction_meets_field_completeness_gate_on_labelled_fixtures() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return;
    };
    let llm = AnthropicProvider::new(api_key);

    let mut correct_classifications = 0usize;
    let mut completeness_scores: Vec<f64> = Vec::new();

    for fixture in FIXTURES {
        let result = extract_entities(fixture.text, "en", &llm).await;
        let qualified = matches!(result, Ok(Some(_)));

        if qualified == fixture.should_qualify {
            correct_classifications += 1;
        }

        if let Ok(Some(extraction)) = &result {
            let score = field_completeness(extraction, fixture);
            if score < 1.0 {
                eprintln!(
                    "incomplete ({:.0}%): {:?} | name={:?} addr={:?} type={:?} scale=({:?},{:?},{:?}) status={:?}",
                    score * 100.0,
                    &fixture.text[..fixture.text.len().min(60)],
                    extraction.project_name,
                    extraction.civic_address,
                    extraction.project_type,
                    extraction.scale_units,
                    extraction.scale_gfa_sqm,
                    extraction.scale_storeys,
                    extraction.approval_status_raw,
                );
            }
            completeness_scores.push(score);
        }
    }

    let classification_accuracy = correct_classifications as f64 / FIXTURES.len() as f64;
    eprintln!(
        "classification accuracy: {:.1}% ({correct_classifications}/{})",
        classification_accuracy * 100.0,
        FIXTURES.len()
    );

    assert!(
        !completeness_scores.is_empty(),
        "expected at least one qualifying extraction to measure completeness against"
    );
    let avg_completeness: f64 = completeness_scores.iter().sum::<f64>() / completeness_scores.len() as f64;
    eprintln!("average field completeness on qualifying extractions: {:.1}%", avg_completeness * 100.0);

    assert!(
        avg_completeness >= 0.90,
        "field completeness {avg_completeness:.2} is below the 90% interim launch gate"
    );
}
