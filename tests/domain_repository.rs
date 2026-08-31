//! Goal: prove this service's own candidate-parsing idempotency entirely
//! without a live infernal-law kernel and without
//! infernal-pf2e-rules-simple. Nothing here signs a request or knows
//! infernal-law exists.

use infernal_pf2e_parser_simple::database::Database;
use infernal_pf2e_parser_simple::domain::{CandidateError, CandidateRepository, SourceReference};
use infernal_pf2e_parser_simple::postgres_candidate_repository::PostgresCandidateRepository;
use uuid::Uuid;

fn repository() -> PostgresCandidateRepository {
    let database = Database::connect_from_env().expect("database should connect and migrate");
    PostgresCandidateRepository::new(database)
}

fn source(document_id: Uuid, version: i64, digest: [u8; 32], text: &str) -> SourceReference {
    SourceReference {
        document_id,
        document_version: version,
        content_digest: digest,
        location: "p1".to_owned(),
        source_text: text.to_owned(),
    }
}

const PARSER_VERSION: &str = "pf2e-parser-test";

#[test]
#[ignore = "requires PF2E_PARSER_DATABASE_URL and PostgreSQL"]
fn a_new_parse_persists_candidates_with_a_stable_id() {
    let repository = repository();
    let document_id = Uuid::new_v4();

    let outcome = repository
        .parse_once(
            PARSER_VERSION,
            source(
                document_id,
                1,
                [1_u8; 32],
                "Action\nName: Stride\nEffect: Move.",
            ),
        )
        .unwrap();

    assert!(!outcome.was_already_processed);
    assert_eq!(outcome.candidates.len(), 1);
    assert_eq!(outcome.candidates[0].name.as_deref(), Some("Stride"));
}

#[test]
#[ignore = "requires PF2E_PARSER_DATABASE_URL and PostgreSQL"]
fn repeated_parsing_of_the_same_source_and_parser_version_reuses_the_same_candidate_id() {
    // The critical property this repository exists to guarantee: a
    // retried pf2e.parse (a reclaimed route, or a fresh resubmission)
    // must reproduce the *same* candidate_id, since
    // infernal-pf2e-rules-simple's own admission idempotency depends on
    // it to recognize a retried admission rather than creating a
    // duplicate rule -- see domain.rs's own module documentation.
    let repository = repository();
    let document_id = Uuid::new_v4();
    let digest = [2_u8; 32];
    let text = "Feat\nName: Toughness\nEffect: More HP.";

    let first = repository
        .parse_once(PARSER_VERSION, source(document_id, 1, digest, text))
        .unwrap();
    let retried = repository
        .parse_once(PARSER_VERSION, source(document_id, 1, digest, text))
        .unwrap();

    assert!(retried.was_already_processed);
    assert_eq!(
        retried.candidates[0].candidate_id,
        first.candidates[0].candidate_id
    );
}

#[test]
#[ignore = "requires PF2E_PARSER_DATABASE_URL and PostgreSQL"]
fn a_new_parser_version_over_the_same_source_produces_a_new_candidate_id() {
    let repository = repository();
    let document_id = Uuid::new_v4();
    let digest = [3_u8; 32];
    let text = "Condition\nName: Prone\nEffect: Flat-footed.";

    let v1 = repository
        .parse_once("pf2e-parser-v1", source(document_id, 1, digest, text))
        .unwrap();
    let v2 = repository
        .parse_once("pf2e-parser-v2", source(document_id, 1, digest, text))
        .unwrap();

    assert!(!v2.was_already_processed);
    assert_ne!(v2.candidates[0].candidate_id, v1.candidates[0].candidate_id);
}

#[test]
#[ignore = "requires PF2E_PARSER_DATABASE_URL and PostgreSQL"]
fn the_same_document_version_with_a_different_digest_is_rejected_as_a_mismatch() {
    let repository = repository();
    let document_id = Uuid::new_v4();
    let text = "Feat\nName: Something\nEffect: Anything.";

    repository
        .parse_once(PARSER_VERSION, source(document_id, 1, [5_u8; 32], text))
        .unwrap();

    let result = repository.parse_once(PARSER_VERSION, source(document_id, 1, [6_u8; 32], text));

    assert!(matches!(result, Err(CandidateError::SourceDigestMismatch)));
}

#[test]
#[ignore = "requires PF2E_PARSER_DATABASE_URL and PostgreSQL"]
fn empty_source_text_is_rejected_before_anything_is_persisted() {
    let repository = repository();

    let result =
        repository.parse_once(PARSER_VERSION, source(Uuid::new_v4(), 1, [7_u8; 32], "   "));

    assert!(matches!(result, Err(CandidateError::EmptySourceText)));
}
