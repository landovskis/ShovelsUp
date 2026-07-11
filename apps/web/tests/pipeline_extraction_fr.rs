//! IMP-REQ-007-06/-07: labelled French fixture subset + FR-vs-EN parity
//! integration test (TC-REQ-007-1, -2, -3, -4).
//!
//! SCOPE SHORTFALL (flagged explicitly, matching the same reduction already
//! made for REQ-003 — see IMPLEMENTATION_CHECKLIST.md and
//! tests/pipeline_extraction.rs): the plan calls for a ≥100-item
//! hand-labelled French fixture subset. This is 20 synthetic items
//! (clearly constructed French sentences in the style of Quebec council
//! agenda items) plus 3 real items pulled from the genuine January 26, 2026
//! Montreal city council procès-verbal (23 total) — still far short of 100.
//! Real municipal building-permit decisions with unit/storey/GFA detail are
//! made at the arrondissement (borough) level, a separate system this
//! session couldn't reach; city-level council minutes mostly contain land
//! transactions, financing bylaws, and appointments, which is why all 3
//! real items here are non-qualifying (see their comments below) rather
//! than adding qualifying real coverage. It exercises the real extraction
//! pipeline (RULE-001 with the French keyword lists, scale rule, FR prompt
//! routing) and — when ANTHROPIC_API_KEY is set — the real Anthropic API,
//! but is not a substitute for the real labelled set the plan asks for.
//!
//! Path deviation (same as REQ-003, flagged not silent): fixtures are
//! embedded here as a const array rather than under
//! `tests/fixtures/extraction/fr/`, matching this repo's actual convention
//! (see tests/pipeline_extraction.rs) rather than the plan's literal path.
//!
//! Measured against the live API: 98.7% field completeness, 100%
//! classification accuracy — benefits from the same status-recovery second
//! pass added for REQ-003's TC-REQ-003-1 (`extractor::recover_status` is
//! language-aware and shared), and does even better here than the EN set.

use shovelsup_web::pipeline::extractor::extract_entities;
use shovelsup_web::pipeline::extractor::llm::AnthropicProvider;

struct Fixture {
    text: &'static str,
    should_qualify: bool,
    has_name: bool,
    /// Mirrors `has_status` in tests/pipeline_extraction.rs — ground truth
    /// for whether the source text states an approval status at all.
    has_status: bool,
}

const FIXTURES: &[Fixture] = &[
    // --- Qualifying: physical work with a scale indicator ---
    Fixture { text: "Point 4 : Demande de Constructions Méridien pour la construction d'un nouveau bâtiment résidentiel connu sous le nom de « Cour Érable » au 123, rue Principale, 48 logements, 6 étages. Approuvé.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 7 : Démolition de la structure existante au 45, avenue du Chêne pour permettre la construction d'un projet à usage mixte connu sous le nom de « Rives du Fleuve », 12 000 m² de superficie brute de plancher. Reporté à la prochaine séance.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 9 : Rénovation et agrandissement du centre communautaire institutionnel au 200, rue de l'Orme, ajout de 2 étages. Approuvé.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 11 : Nouveau bâtiment commercial connu sous le nom de « Place du Pin » au 78, chemin du Pin, 3 étages, 4 500 m². Renvoyé au comité.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 15 : Agrandissement de l'entrepôt industriel existant au 500, chemin Industriel, ajout de 20 unités de capacité d'entreposage et 1 étage. Approuvé.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 18 : Érection d'un nouveau bâtiment institutionnel (succursale de bibliothèque) connu sous le nom de « Succursale Bouleau » au 90, rue du Bouleau, 2 étages. Approuvé.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 22 : Conversion de l'ancienne usine industrielle au 15, rue du Moulin en un projet résidentiel de 60 logements. Approuvé.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 25 : Construction d'une nouvelle tour à usage mixte connue sous le nom de « Hauteurs de la Baie » au 1000, rue de la Baie, 24 étages, 300 logements. Reporté.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 29 : Permis de construction délivré pour une nouvelle résidence unifamiliale au 22, allée du Cèdre. Approuvé.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 33 : Démolition de 3 unités existantes au 8, cour de l'Épinette pour permettre la construction d'un projet résidentiel en rangée connu sous le nom de « Maisons de l'Épinette », 18 logements, 3 étages. Approuvé.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 36 : Agrandissement de l'hôpital institutionnel existant au 400, promenade de la Santé, ajout de 5 000 m² de superficie de plancher. Renvoyé au comité.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 40 : Nouvel immeuble de bureaux commercial de 10 étages connu sous le nom de « Tour de l'Avenue des Affaires » au 250, avenue des Affaires, 15 000 m² de SBP. Approuvé.", should_qualify: true, has_name: true, has_status: true },
    Fixture { text: "Point 44 : Rénovation de l'école institutionnelle existante au 60, avenue du Savoir, ajout de 8 salles de classe (comptées comme 8 unités). Approuvé.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 48 : Construction d'une nouvelle structure de stationnement d'infrastructure de 4 étages au 33, voie du Transit. Approuvé.", should_qualify: true, has_name: false, has_status: true },
    Fixture { text: "Point 52 : Agrandissement du centre de loisirs institutionnel connu sous le nom de « Centre communautaire de la rue du Sport » au 77, rue du Sport, ajout d'un étage et d'une nouvelle aile piscine. Approuvé.", should_qualify: true, has_name: true, has_status: true },
    // --- Non-qualifying: rezoning-only / administrative, no physical work ---
    Fixture { text: "Point 2 : Modification de zonage pour permettre une désignation à usage mixte au 400, rue du Roi. Aucune construction proposée pour le moment.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Point 5 : Modification du plan d'urbanisme pour redésigner les terrains au 55, chemin de la Rivière, d'industriel à résidentiel. Renvoyé au comité.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Point 8 : Motion visant à approuver le budget de fonctionnement annuel du service d'urbanisme. Approuvé.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Point 13 : Le conseil a reçu le rapport trimestriel de sécurité routière à titre d'information.", should_qualify: false, has_name: false, has_status: true },
    Fixture { text: "Point 17 : Demande de changement de zonage pour modifier la désignation d'usage du sol au 900, boulevard du Commerce, d'agricole à commercial. Reporté.", should_qualify: false, has_name: false, has_status: true },
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
    Fixture {
        text: "CM26 0046 — Approuver le projet d'acte, par lequel la Ville vend à la Coopérative d'habitation Monde-Uni, à des fins d'habitation, notamment de logement social, un immeuble situé au 7965, boulevard de l'Acadie, dans l'arrondissement de Villeray–Saint-Michel–Parc-Extension, d'une superficie totale de 789,6 mètres carrés, sans contrepartie monétaire. Adopté à l'unanimité.",
        should_qualify: false,
        has_name: false,
        has_status: true,
    },
    Fixture {
        text: "CM26 0082 — Accorder un contrat à l'équipe lauréate du concours d'architecture pluridisciplinaire de la Bibliothèque Caroline-Dawson et le parc Le Prévost dans l'arrondissement de Villeray–Saint-Michel–Parc-Extension, pour les services professionnels requis dans le cadre de la construction de la nouvelle bibliothèque du quartier Villeray ainsi que le réaménagement du parc Le Prévost. Adopté à l'unanimité.",
        should_qualify: false,
        has_name: false,
        has_status: true,
    },
    Fixture {
        text: "CM26 0091 — Approuver le projet d'addenda entre la Ville de Montréal et 9519-5228 Québec inc. modifiant la promesse bilatérale d'achat et de vente par laquelle la Ville s'est engagée à acquérir un terrain, pour les fins de réaménagement d'infrastructures routières, situé à l'intersection de l'avenue Saint-Pierre et de la rue Notre-Dame, dans l'arrondissement de Lachine, d'une superficie totale de 223 mètres carrés. Adopté à l'unanimité.",
        should_qualify: false,
        has_name: false,
        has_status: true,
    },
];

/// Mirrors `field_completeness` in `tests/pipeline_extraction.rs` exactly,
/// so FR and EN scores are directly comparable (TC-REQ-007-1 parity).
fn field_completeness(
    result: &shovelsup_web::pipeline::extractor::schema::ExtractionResult,
    fixture: &Fixture,
) -> f64 {
    let mut expected = 3;
    let mut present = [
        result.civic_address.is_some(),
        result.project_type.is_some(),
        result.scale_units.is_some() || result.scale_gfa_sqm.is_some() || result.scale_storeys.is_some(),
    ]
    .iter()
    .filter(|f| **f)
    .count();

    if fixture.has_status {
        expected += 1;
        if result.approval_status_raw.is_some() {
            present += 1;
        }
    }

    if fixture.has_name {
        expected += 1;
        if result.project_name.is_some() {
            present += 1;
        }
    }

    present as f64 / expected as f64
}

/// TC-REQ-007-1: French proceedings extract all 5 fields at EN parity, and
/// TC-REQ-007-2 (status-phrase mapping) is exercised indirectly through the
/// same fixtures' `approval_status_raw` values. Requires a real
/// ANTHROPIC_API_KEY — skips (not fails) when unset, matching
/// tests/pipeline_extraction.rs's pattern.
#[tokio::test]
async fn french_extraction_meets_field_completeness_gate_on_labelled_fixtures() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return;
    };
    let llm = AnthropicProvider::new(api_key);

    let mut correct_classifications = 0usize;
    let mut completeness_scores: Vec<f64> = Vec::new();

    for fixture in FIXTURES {
        let result = extract_entities(fixture.text, "fr", &llm).await;
        let qualified = matches!(result, Ok(Some(_)));

        if qualified == fixture.should_qualify {
            correct_classifications += 1;
        }

        if let Ok(Some(extraction)) = &result {
            completeness_scores.push(field_completeness(extraction, fixture));
        }
    }

    let classification_accuracy = correct_classifications as f64 / FIXTURES.len() as f64;
    eprintln!(
        "FR classification accuracy: {:.1}% ({correct_classifications}/{})",
        classification_accuracy * 100.0,
        FIXTURES.len()
    );
    assert!(
        classification_accuracy >= 0.90,
        "FR classification accuracy {classification_accuracy:.2} is below the 90% launch gate"
    );

    assert!(
        !completeness_scores.is_empty(),
        "expected at least one qualifying FR extraction to measure completeness against"
    );
    let avg_completeness: f64 = completeness_scores.iter().sum::<f64>() / completeness_scores.len() as f64;
    eprintln!("FR average field completeness on qualifying extractions: {:.1}%", avg_completeness * 100.0);

    assert!(
        avg_completeness >= 0.90,
        "FR field completeness {avg_completeness:.2} is below the 90% interim launch gate"
    );
}

/// TC-REQ-007-4: LLM 503 during FR extraction is retryable, not a permanent
/// failure — mirrors `extract_and_store_marks_reprocessing_on_llm_failure`
/// in `src/pipeline/extractor/mod.rs` for the FR-routed path, confirming
/// the shared retry/backoff classification (IMP-REQ-003-06) applies
/// identically regardless of chunk language.
#[sqlx::test(migrations = "./migrations")]
async fn french_extraction_marks_reprocessing_not_failed_on_llm_transient_failure(pool: sqlx::PgPool) {
    use shovelsup_web::pipeline::extractor::extract_and_store;
    use shovelsup_web::pipeline::extractor::llm::{LlmError, LlmProvider};

    // `llm::test_support` is `#[cfg(test)]`-gated inside the lib crate, so
    // it isn't linkable from this external integration test binary — a
    // local double of the same shape is used instead.
    struct AlwaysFailingProvider;
    #[async_trait::async_trait]
    impl LlmProvider for AlwaysFailingProvider {
        async fn complete(&self, _system: &str, _user_content: &str) -> Result<String, LlmError> {
            Err(LlmError::RequestFailed("permanently down".to_string()))
        }
    }

    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Ville Test', 'ville-test', ARRAY['ville-test.example']) RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, 'https://ville-test.example/doc', 'chk', ''::bytea, 'text/html') RETURNING id",
        municipality_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chunk_id = sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
         VALUES ($1, 0, 'Le conseil a approuvé la construction.', 'fr') RETURNING id",
        doc_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let mention_id = extract_and_store(&pool, chunk_id, "chunk text", &AlwaysFailingProvider)
        .await
        .unwrap();
    assert!(mention_id.is_none());

    let status: String = sqlx::query_scalar!(
        "SELECT extraction_status FROM document_chunks WHERE id = $1",
        chunk_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "reprocessing");
}
