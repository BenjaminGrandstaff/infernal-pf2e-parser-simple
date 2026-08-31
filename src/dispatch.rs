//! Goal: interpret a routed `pf2e.parse` Request, produce candidates, and
//! hand each one to `infernal-pf2e-rules-simple` via a governed
//! `pf2e.rules.admit` Request of this service's own -- the boundary
//! between "the kernel handed us governed work" and "this service's own
//! domain knows what to do with it, including how to hand its result to
//! the next service." Nothing below this module knows infernal-law
//! exists; nothing above it knows what a PF2e rule is.
//!
//! ## The kernel's Request has no payload field -- on both sides of this service
//!
//! infernal-law's MVP `Request` carries only a namespaced `action`, a
//! `scope` string bounded to 200 characters, and schema version
//! references -- ILK-006 artifact/content mediation is Future Kernel, not
//! built. This service inherits the same constraint
//! `infernal-rules-extractor-pf2e` documented first, on *both* of its
//! governed hops:
//!
//! - **input** (`pf2e.parse`, received): `scope` is
//!   `<document_id>@<document_version>#<location>!<content_digest_b64url>|<source_text>`
//!   -- identical wire shape to the earlier single-service design.
//! - **output** (`pf2e.rules.admit`, submitted): `scope` is
//!   `<candidate_id>@<parser_version>#<rule_type>!<confidence>|<document_id>~<document_version>~<digest_b64>~<location>~<name>`
//!   -- candidate identity, classification, confidence, and full source
//!   provenance, but *not* the candidate's normalized fields (`trigger`,
//!   `requirements`, `prerequisites`, `effect`, `references`): there is
//!   no room left in 200 characters once exact provenance (a full
//!   32-byte digest) is included. See this repository's README, "Kernel
//!   payload limitations", for the fuller accounting -- this is not
//!   routed around with a side channel to the Rules Service.

use uuid::Uuid;

use crate::domain::{Candidate, CandidateRepository, SourceReference};
use crate::error::ParserError;
use crate::kernel_client::KernelPort;

pub const PARSE_ACTION: &str = "pf2e.parse";
pub const ADMIT_ACTION: &str = "pf2e.rules.admit";

/// Every action this service subscribes to and performs.
pub const ACTIONS: [&str; 1] = [PARSE_ACTION];

pub const PARSER_VERSION: &str = concat!("pf2e-parser-", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub enum DispatchOutcome {
    Parsed {
        candidate_count: usize,
        was_already_processed: bool,
    },
}

pub fn dispatch(
    action: &str,
    scope: &str,
    repository: &dyn CandidateRepository,
    kernel: &dyn KernelPort,
) -> Result<DispatchOutcome, ParserError> {
    match action {
        PARSE_ACTION => {
            let source = parse_input_scope(scope)?;
            let outcome = repository.parse_once(PARSER_VERSION, source)?;
            for candidate in &outcome.candidates {
                let admit_scope = admit_scope_for(&outcome.source, candidate);
                kernel.submit_request(ADMIT_ACTION, &admit_scope)?;
            }
            Ok(DispatchOutcome::Parsed {
                candidate_count: outcome.candidates.len(),
                was_already_processed: outcome.was_already_processed,
            })
        }
        other => Err(ParserError::UnknownAction(other.to_owned())),
    }
}

fn parse_input_scope(scope: &str) -> Result<SourceReference, ParserError> {
    let (header, source_text) = scope.split_once('|').ok_or(ParserError::MalformedScope(
        "scope must contain '|' separating the header from source text",
    ))?;
    let (id_version_location, digest_b64) = header.split_once('!').ok_or(
        ParserError::MalformedScope("scope header must contain '!' before the content digest"),
    )?;
    let (id_version, location) =
        id_version_location
            .split_once('#')
            .ok_or(ParserError::MalformedScope(
                "scope header must contain '#' before the location",
            ))?;
    let (document_id, version) = id_version
        .split_once('@')
        .ok_or(ParserError::MalformedScope(
            "scope header must contain '@' before the version",
        ))?;

    let document_id: Uuid = document_id
        .parse()
        .map_err(|_| ParserError::MalformedScope("document_id must be a UUID"))?;
    let document_version: i64 = version
        .parse()
        .map_err(|_| ParserError::MalformedScope("document_version must be an integer"))?;
    if location.is_empty() {
        return Err(ParserError::MalformedScope("location must not be empty"));
    }
    let content_digest = decode_digest(digest_b64)?;

    Ok(SourceReference {
        document_id,
        document_version,
        content_digest,
        location: location.to_owned(),
        source_text: source_text.to_owned(),
    })
}

fn admit_scope_for(source: &SourceReference, candidate: &Candidate) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}@{}#{}!{:.2}|{}~{}~{}~{}~{}",
        candidate.candidate_id,
        PARSER_VERSION,
        candidate.rule_type.as_str(),
        candidate.confidence,
        source.document_id,
        source.document_version,
        URL_SAFE_NO_PAD.encode(source.content_digest),
        source.location,
        candidate.name.as_deref().unwrap_or(""),
    )
}

fn decode_digest(value: &str) -> Result<[u8; 32], ParserError> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ParserError::MalformedScope("content digest must be base64url"))?;
    bytes
        .try_into()
        .map_err(|_| ParserError::MalformedScope("content digest must be exactly 32 bytes"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use crate::domain::{CandidateError, ParseOutcome, RuleType};

    use super::*;

    #[derive(Default)]
    struct FakeRepository {
        result: Option<Result<ParseOutcome, CandidateError>>,
    }

    impl CandidateRepository for FakeRepository {
        fn parse_once(
            &self,
            _parser_version: &str,
            source: SourceReference,
        ) -> Result<ParseOutcome, CandidateError> {
            match &self.result {
                Some(Ok(outcome)) => Ok(ParseOutcome {
                    source,
                    candidates: outcome.candidates.clone(),
                    was_already_processed: outcome.was_already_processed,
                }),
                Some(Err(error)) => Err(*error),
                None => Ok(ParseOutcome {
                    source,
                    candidates: vec![sample_candidate()],
                    was_already_processed: false,
                }),
            }
        }
    }

    fn sample_candidate() -> Candidate {
        Candidate {
            candidate_id: Uuid::new_v4(),
            rule_type: RuleType::Action,
            name: Some("Stride".to_owned()),
            traits: Vec::new(),
            trigger: None,
            requirements: Vec::new(),
            prerequisites: Vec::new(),
            effect: Some("Move.".to_owned()),
            references: Vec::new(),
            source_text: "Action\nName: Stride\nEffect: Move.".to_owned(),
            inferences: Vec::new(),
            confidence: 0.9,
        }
    }

    #[derive(Default)]
    struct FakeKernel {
        submissions: Mutex<Vec<(String, String)>>,
        submission_result: Option<Result<(), ParserError>>,
    }

    impl KernelPort for FakeKernel {
        fn eligible_routes(&self) -> Result<Vec<crate::routes::EligibleRoute>, ParserError> {
            unimplemented!("not exercised by dispatch tests")
        }

        fn propose_claim(
            &self,
            _route_id: &str,
            _lease_seconds: i64,
        ) -> Result<crate::claims::ClaimOutcome, ParserError> {
            unimplemented!("not exercised by dispatch tests")
        }

        fn routed_request(
            &self,
            _route_id: &str,
        ) -> Result<crate::routed_request::RoutedRequestOutcome, ParserError> {
            unimplemented!("not exercised by dispatch tests")
        }

        fn complete_claim(
            &self,
            _claim_id: &str,
            _fencing_token: i64,
        ) -> Result<crate::claims::CompleteOutcome, ParserError> {
            unimplemented!("not exercised by dispatch tests")
        }

        fn submit_request(&self, action: &str, scope: &str) -> Result<(), ParserError> {
            self.submissions
                .lock()
                .unwrap()
                .push((action.to_owned(), scope.to_owned()));
            match &self.submission_result {
                Some(Ok(())) | None => Ok(()),
                Some(Err(ParserError::AdmissionNotAccepted(status))) => {
                    Err(ParserError::AdmissionNotAccepted(*status))
                }
                Some(Err(_)) => unreachable!(),
            }
        }
    }

    fn scope_for(
        document_id: Uuid,
        version: i64,
        location: &str,
        digest: [u8; 32],
        text: &str,
    ) -> String {
        format!(
            "{document_id}@{version}#{location}!{}|{text}",
            URL_SAFE_NO_PAD.encode(digest)
        )
    }

    #[test]
    fn a_well_formed_scope_is_parsed_and_the_candidate_is_submitted_for_admission() {
        let repository = FakeRepository::default();
        let kernel = FakeKernel::default();
        let scope = scope_for(
            Uuid::new_v4(),
            1,
            "p1",
            [0; 32],
            "Action\nName: Stride\nEffect: Move.",
        );

        let outcome = dispatch(PARSE_ACTION, &scope, &repository, &kernel).unwrap();

        assert!(matches!(
            outcome,
            DispatchOutcome::Parsed {
                candidate_count: 1,
                was_already_processed: false
            }
        ));
        let submissions = kernel.submissions.lock().unwrap();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].0, ADMIT_ACTION);
        assert!(submissions[0].1.contains("Stride"));
        assert!(submissions[0].1.contains("action"));
    }

    #[test]
    fn an_unconfirmed_admission_propagates_as_an_error() {
        let repository = FakeRepository::default();
        let kernel = FakeKernel {
            submission_result: Some(Err(ParserError::AdmissionNotAccepted(503))),
            ..FakeKernel::default()
        };
        let scope = scope_for(
            Uuid::new_v4(),
            1,
            "p1",
            [0; 32],
            "Action\nName: Stride\nEffect: Move.",
        );

        let result = dispatch(PARSE_ACTION, &scope, &repository, &kernel);

        assert!(matches!(
            result,
            Err(ParserError::AdmissionNotAccepted(503))
        ));
    }

    #[test]
    fn a_malformed_scope_is_rejected_before_touching_the_repository_or_kernel() {
        let repository = FakeRepository::default();
        let kernel = FakeKernel::default();

        let result = dispatch(PARSE_ACTION, "not-a-valid-scope", &repository, &kernel);

        assert!(matches!(result, Err(ParserError::MalformedScope(_))));
        assert!(kernel.submissions.lock().unwrap().is_empty());
    }

    #[test]
    fn unknown_actions_are_rejected_without_touching_the_repository() {
        let repository = FakeRepository::default();
        let kernel = FakeKernel::default();

        let result = dispatch("pf2e.parse.validate", "irrelevant", &repository, &kernel);

        match result {
            Err(ParserError::UnknownAction(action)) => assert_eq!(action, "pf2e.parse.validate"),
            other => panic!("expected UnknownAction, got {other:?}"),
        }
    }
}
