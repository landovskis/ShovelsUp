//! IMP-REQ-004-07: bilingual parity across launch municipalities.
//! Acceptance criteria: >=90% non-null status across EN+FR fixtures.
//! Deterministic (DB vocabulary lookup, no LLM call) — always runs, no
//! ANTHROPIC_API_KEY gate needed.

use shovelsup_web::pipeline::normalizer::normalize_status;
use sqlx::PgPool;

const EN_PHRASES: &[&str] = &[
    "Approved.",
    "The application was approved unanimously.",
    "Council adopted the motion.",
    "Deferred to next meeting.",
    "The item was postponed pending further study.",
    "Referred to committee.",
    "This matter was referred to the planning committee for review.",
    "Rejected.",
    "The application was denied.",
    "Proposed for consideration at a future meeting.",
    "The application was submitted for council's review.",
];

const FR_PHRASES: &[&str] = &[
    "Approuvé.",
    "La demande a été approuvée à l'unanimité.",
    "Le conseil a adopté la motion.",
    "Reporté à la prochaine séance.",
    "L'article a été différé en attendant une étude plus approfondie.",
    "Référé au comité.",
    "Cette question a été renvoyée au comité d'urbanisme pour examen.",
    "Rejeté.",
    "La demande a été refusée.",
    "Proposé pour examen lors d'une réunion future.",
    "La demande a été soumise pour examen par le conseil.",
];

#[sqlx::test(migrations = "./migrations")]
async fn bilingual_status_normalization_meets_parity_gate(pool: PgPool) {
    let mut non_null_en = 0usize;
    for phrase in EN_PHRASES {
        if normalize_status(&pool, phrase, "en").await.unwrap().is_some() {
            non_null_en += 1;
        }
    }

    let mut non_null_fr = 0usize;
    for phrase in FR_PHRASES {
        if normalize_status(&pool, phrase, "fr").await.unwrap().is_some() {
            non_null_fr += 1;
        }
    }

    let en_rate = non_null_en as f64 / EN_PHRASES.len() as f64;
    let fr_rate = non_null_fr as f64 / FR_PHRASES.len() as f64;

    assert!(
        en_rate >= 0.90,
        "EN non-null rate {en_rate:.2} below 90% gate ({non_null_en}/{})",
        EN_PHRASES.len()
    );
    assert!(
        fr_rate >= 0.90,
        "FR non-null rate {fr_rate:.2} below 90% gate ({non_null_fr}/{})",
        FR_PHRASES.len()
    );
}

/// The same phrase pattern in EN and FR must resolve to the same
/// normalized_status value — the actual definition of "parity", not just
/// "both languages hit some threshold independently".
#[sqlx::test(migrations = "./migrations")]
async fn matching_en_fr_phrase_pairs_resolve_to_the_same_status(pool: PgPool) {
    let pairs = [
        ("Approved.", "Approuvé."),
        ("Deferred.", "Reporté."),
        ("Referred to committee.", "Référé au comité."),
        ("Rejected.", "Rejeté."),
        ("Proposed.", "Proposé."),
    ];

    for (en, fr) in pairs {
        let en_status = normalize_status(&pool, en, "en").await.unwrap();
        let fr_status = normalize_status(&pool, fr, "fr").await.unwrap();
        assert_eq!(en_status, fr_status, "EN {en:?} and FR {fr:?} should resolve to the same status");
        assert!(en_status.is_some(), "expected {en:?} to resolve, got None");
    }
}
