//! IMP-REQ-003-08/09: labelled fixture set + field-completeness integration
//! test (TC-REQ-003-1, and the field-completeness gate from Autonomous
//! Execution Notes: "use the ≥90% field-completeness metric as the interim
//! launch gate").
//!
//! SCOPE SHORTFALL (flagged explicitly, not silently shipped): the plan
//! calls for a ≥200-item hand-labelled fixture set across 3 municipalities.
//! This is 30 synthetic items (clearly-constructed English sentences in the
//! style of council agenda items) plus 3 real items pulled directly from
//! real City of Toronto "Report for Action" documents (33 total) — still
//! far short of 200. Real attempt made: Toronto's toronto.ca/legdocs is
//! genuinely reachable and yielded real addresses/scale (see the REAL
//! FIXTURES block below); Vancouver's council.vancouver.ca and
//! rezoning.vancouver.ca returned HTTP 403 on every path tried (WebFetch
//! and direct curl with a browser user agent) — completely inaccessible in
//! this session, not a scope choice. Montreal's real items live in
//! tests/pipeline_extraction_fr.rs. See IMPLEMENTATION_CHECKLIST.md REQ-003
//! risks for the full account.
//!
//! RESOLVED (previously an unresolved finding, ~85% completeness — see git
//! history for the original note): the gap was entirely in
//! approval_status_raw going null on ~25% of qualifying extractions despite
//! six rounds of prompt-only iteration. Root cause, confirmed directly
//! against the live API rather than assumed: asking for 9 fields in one
//! structured-output call made this one short trailing-sentence field
//! disproportionately likely to be dropped. Fixed with a second-pass,
//! status-only call (`extractor::recover_status`) that fires only when the
//! main call returns null for it — see `extractor::extract_entities` and
//! `llm::LlmProvider::complete_text`. Two dead ends ruled out along the
//! way: `temperature` is outright rejected by this model's API as
//! deprecated (not just a bad idea); the first version of the recovery
//! pass reused `complete()`, whose JSON-schema output constraint made the
//! model re-emit a full extraction object as the "status" instead of plain
//! text — fixed by adding `complete_text` (no schema constraint).
//! Current measured completeness: 95.3%, classification accuracy 100%.

use shovelsup_pipeline::extractor::extract_entities;
use shovelsup_pipeline::extractor::llm::AnthropicProvider;

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
    /// Ground truth: does the fixture text actually state an approval
    /// status? True for every synthetic fixture (constructed with a
    /// trailing decision phrase); false for the real-document fixtures
    /// below whose source is a pre-decision staff report with no recorded
    /// vote outcome — scoring those as "missing" a field their source
    /// genuinely doesn't contain would be the same double-penalty
    /// `has_name` already guards against for project_name.
    has_status: bool,
}

const FIXTURES: &[Fixture] = &[
    // --- Qualifying: physical work with a scale indicator. Every fixture
    // states an address, type, scale, and approval status; only project
    // name legitimately varies (see `has_name` and `field_completeness`). ---
    Fixture { text: "Item 4: Application by Meridian Homes for construction of a new residential building known as \"Maple Court\" at 123 Main St, 48 units, 6 storeys. Approved.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 7: Demolition of the existing structure at 45 Oak Ave to permit construction of a mixed-use development known as \"Riverside Commons\", 12,000 sqm gross floor area. Deferred to next meeting.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 9: Renovation and addition to the institutional community centre at 200 Elm St, adding 2 storeys. Approved.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 11: New commercial building known as \"Pine Road Plaza\" at 78 Pine Rd, 3 storeys, 4,500 sqm. Referred to committee.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 15: Expansion of the existing industrial warehouse at 500 Industrial Way, adding 20 units of storage capacity and 1 storey. Approved.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 18: Erection of a new institutional building (library branch) known as the \"Birch Street Branch\" at 90 Birch St, 2 storeys. Approved.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 22: Conversion of the former industrial factory at 15 Mill St into a residential development of 60 units. Approved.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 25: Construction of a new mixed-use tower known as \"Bay Street Heights\" at 1000 Bay St, 24 storeys, 300 units. Deferred.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 29: Building permit issued for a new residential single-family dwelling at 22 Cedar Lane. Approved.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 33: Demolition of 3 existing units at 8 Spruce Ct to permit construction of a residential townhouse development known as \"Spruce Court Towns\", 18 units, 3 storeys. Approved.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 36: Addition to the existing institutional hospital at 400 Health Dr, adding 5,000 sqm of floor area. Referred to committee.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 40: New 10-storey commercial office building known as \"Business Parkway Tower\" at 250 Business Pkwy, 15,000 sqm GFA. Approved.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Item 44: Renovation of the existing institutional school at 60 Learning Ave, adding 8 classrooms (treated as 8 units). Approved.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 48: Construction of a new 4-storey infrastructure parking structure at 33 Transit Way. Approved.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Item 52: Expansion of the institutional recreation centre known as \"Sport Street Community Centre\" at 77 Sport St, adding 1 storey and a new pool wing. Approved.", should_qualify: true, has_name: true, has_status: true },
    // --- REAL FIXTURES (IMP-REQ-003-08): sourced from real City of Toronto
    // "Report for Action" staff reports fetched from toronto.ca/legdocs.
    // These are pre-decision reports (not post-vote minutes), so they
    // genuinely have no recorded approval_status_raw — has_status: false
    // reflects that ground truth rather than penalizing a correct null.
    // Retrieved 2026-07-11.
    Fixture {
        text: "Residential Demolition Applications – 46, 48, 50 and 52 Laing Street. Applications for the demolition of the existing vacant residential buildings at 46, 48, 50 and 52 Laing Street were submitted to Toronto Building. A building permit application to construct a new eight-storey residential building with 248 rental units has been received. The owner has indicated they wish to demolish the buildings at 46, 48, 50 and 52 Laing Street to ensure the site is ready for the proposal to construct a new eight-storey residential building. Source: toronto.ca/legdocs/mmis/2026/te/bgrd/backgroundfile-261199.pdf",
        should_qualify: true,
        has_name: false,
        has_status: false,
    },
    Fixture {
        text: "Toronto Builds - 1-97 Dorney Court, 2-8 Flemington Road and 21-39 Varna Drive – Rental Housing Demolition Application. This report recommends approval of a Rental Housing Demolition application which proposes to demolish 121 existing social housing units within townhouses at 1-97 Dorney Court, four two-storey residential rental apartment buildings at 2-8 Flemington Road and ten single-detached homes at 21-39 Varna Drive. The 175 social housing units are proposed to be replaced by Toronto Community Housing Corporation as part of Phases 2 and 3 of the Lawrence Heights revitalization. Source: toronto.ca/legdocs/mmis/2026/ph/bgrd/backgroundfile-264818.pdf",
        should_qualify: true,
        has_name: false,
        has_status: false,
    },
    Fixture {
        text: "241 Redpath Avenue – Rental Housing Demolition Application – Final Report. The application proposes to demolish a 12-storey apartment building containing 46 rental units located at 241 Redpath Avenue. The 46 rental units are proposed to be replaced as part of the new 38-storey building comprised of 362 dwelling units. This report recommends approval of the Rental Housing Demolition application under Chapter 667 of the Toronto Municipal Code. Source: toronto.ca/legdocs/mmis/2022/ny/bgrd/backgroundfile-226674.pdf",
        should_qualify: true,
        has_name: false,
        has_status: false,
    },
    // --- Non-qualifying: rezoning-only / administrative, no physical work ---
    Fixture { text: "Item 2: Zoning by-law amendment to permit mixed-use designation at 400 King St. No construction proposed at this time.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 5: Official plan amendment to redesignate lands at 55 River Rd from industrial to residential. Referred to committee.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 8: Motion to approve the annual operating budget for the planning department. Approved.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 13: Council received the quarterly traffic safety report for information.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 17: Rezoning application to change land use designation at 900 Commerce Blvd from agricultural to commercial. Deferred.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 20: Appointment of a new member to the heritage advisory committee. Approved.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 24: Council approved the minutes of the previous meeting.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 27: Zoning by-law amendment to update parking requirements city-wide. Approved.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 31: Public consultation scheduled regarding the draft transportation master plan.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 35: Council received a staff report on winter road maintenance for information purposes.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 39: Motion to award the annual snow removal contract. Approved.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 42: Official plan amendment to designate a new employment area at 700 Logistics Dr. Referred to committee.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 46: Council proclaimed the following week as Small Business Week.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 50: Motion to appoint an interim city clerk. Approved.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Item 54: Zoning amendment to permit a home-based business use with no described physical alterations.", should_qualify: false, has_name: false, has_status: true },
];

/// Completeness measured against ground truth: civic_address, project_type,
/// a scale indicator, and approval_status_raw are present in every
/// qualifying fixture's text by construction, so they're always expected.
/// project_name is only expected when `has_name` says the text actually
/// states one — a correct null on a nameless fixture must not count as
/// incomplete.
fn field_completeness(
    result: &shovelsup_pipeline::extractor::schema::ExtractionResult,
    fixture: &Fixture,
) -> f64 {
    let mut expected = 3;
    let mut present = [
        result.civic_address.is_some(),
        result.project_type.is_some(),
        result.scale_units.is_some()
            || result.scale_gfa_sqm.is_some()
            || result.scale_storeys.is_some(),
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

    if fixture.has_status {
        expected += 1;
        if result.approval_status_raw.is_some() {
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
        classification_accuracy >= 0.90,
        "classification accuracy {classification_accuracy:.2} is below the 90% launch gate"
    );

    assert!(
        !completeness_scores.is_empty(),
        "expected at least one qualifying extraction to measure completeness against"
    );
    let avg_completeness: f64 =
        completeness_scores.iter().sum::<f64>() / completeness_scores.len() as f64;
    eprintln!(
        "average field completeness on qualifying extractions: {:.1}%",
        avg_completeness * 100.0
    );

    assert!(
        avg_completeness >= 0.90,
        "field completeness {avg_completeness:.2} is below the 90% interim launch gate"
    );
}
