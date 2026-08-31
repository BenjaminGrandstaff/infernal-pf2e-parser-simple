//! Goal: prove `work_once`'s orchestration of `KernelPort` and
//! `CandidateRepository` against fakes. No live kernel, no live database,
//! no live Rules Service.
//!
//! The critical property this file exists to prove: if submitting the
//! `pf2e.rules.admit` Request is not confirmed, this service's own
//! `pf2e.parse` claim must never be completed -- a route reclaim must be
//! able to retry the whole pass, and `postgres_candidate_repository`'s
//! own candidate_id stability (proven in `tests/domain_repository.rs`) is
//! what keeps that retry from ever producing a duplicate rule at the
//! Rules Service.

use std::sync::Mutex;

use infernal_pf2e_parser_simple::claims::{ClaimOutcome, CompleteOutcome, WorkClaim};
use infernal_pf2e_parser_simple::domain::{
    Candidate, CandidateError, CandidateRepository, ParseOutcome, RuleType, SourceReference,
};
use infernal_pf2e_parser_simple::error::ParserError;
use infernal_pf2e_parser_simple::kernel_client::KernelPort;
use infernal_pf2e_parser_simple::routed_request::RoutedRequestOutcome;
use infernal_pf2e_parser_simple::routes::EligibleRoute;
use infernal_pf2e_parser_simple::{WorkOutcome, work_once};
use uuid::Uuid;

#[derive(Default)]
struct FakePort {
    routes: Vec<EligibleRoute>,
    claim_outcome: Option<ClaimOutcome>,
    request_outcome: Option<RoutedRequestOutcome>,
    complete_outcome: Option<CompleteOutcome>,
    submission_result: Option<Result<(), u16>>,
    complete_calls: Mutex<u32>,
    submit_calls: Mutex<u32>,
}

impl KernelPort for FakePort {
    fn eligible_routes(&self) -> Result<Vec<EligibleRoute>, ParserError> {
        Ok(self.routes.clone())
    }

    fn propose_claim(
        &self,
        _route_id: &str,
        _lease_seconds: i64,
    ) -> Result<ClaimOutcome, ParserError> {
        Ok(self
            .claim_outcome
            .clone()
            .unwrap_or(ClaimOutcome::AlreadyClaimed))
    }

    fn routed_request(&self, _route_id: &str) -> Result<RoutedRequestOutcome, ParserError> {
        Ok(self
            .request_outcome
            .clone()
            .unwrap_or(RoutedRequestOutcome::NotFound))
    }

    fn complete_claim(
        &self,
        _claim_id: &str,
        _fencing_token: i64,
    ) -> Result<CompleteOutcome, ParserError> {
        *self.complete_calls.lock().unwrap() += 1;
        Ok(self
            .complete_outcome
            .clone()
            .unwrap_or(CompleteOutcome::NotFound))
    }

    fn submit_request(&self, _action: &str, _scope: &str) -> Result<(), ParserError> {
        *self.submit_calls.lock().unwrap() += 1;
        match self.submission_result {
            Some(Ok(())) | None => Ok(()),
            Some(Err(status)) => Err(ParserError::AdmissionNotAccepted(status)),
        }
    }
}

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

fn route() -> EligibleRoute {
    EligibleRoute {
        route_id: "route-1".to_owned(),
        request_id: Uuid::new_v4().to_string(),
        subscription_id: "subscription-1".to_owned(),
        destination_service_id: "destination-1".to_owned(),
        created_at: 1,
    }
}

fn claim() -> WorkClaim {
    WorkClaim {
        claim_id: "claim-1".to_owned(),
        route_id: "route-1".to_owned(),
        worker_service_id: "destination-1".to_owned(),
        worker_instance_id: "instance-1".to_owned(),
        fencing_token: 1,
        status: "active".to_owned(),
        claimed_at: 1,
        lease_expires_at: 301,
    }
}

fn routed_request(route: &EligibleRoute, action: &str, scope: &str) -> RoutedRequestOutcome {
    RoutedRequestOutcome::Found(infernal_pf2e_parser_simple::routed_request::RoutedRequest {
        request_id: route.request_id.clone(),
        source_service_id: "source-1".to_owned(),
        action: action.to_owned(),
        scope: scope.to_owned(),
        artifact_schema_version_id: "a1".to_owned(),
        permission_policy_schema_version_id: "p1".to_owned(),
        accepted_at: 1,
    })
}

fn parse_scope() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}@1#p1!{}|Action\nName: Stride\nEffect: Move.",
        Uuid::new_v4(),
        URL_SAFE_NO_PAD.encode([0_u8; 32])
    )
}

#[test]
fn does_nothing_when_no_route_is_eligible() {
    let port = FakePort::default();
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(outcome, WorkOutcome::NothingEligible));
}

#[test]
fn completes_a_full_parse_and_admission_dispatch() {
    let route = route();
    let scope = parse_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.parse", &scope)),
        complete_outcome: Some(CompleteOutcome::Completed(claim())),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(outcome, WorkOutcome::Completed { .. }));
    assert_eq!(*port.submit_calls.lock().unwrap(), 1);
    assert_eq!(*port.complete_calls.lock().unwrap(), 1);
}

#[test]
fn an_unconfirmed_admission_never_completes_this_services_own_claim() {
    // The critical test: "Parser successfully created candidate; Rules
    // Service admission Request was not confirmed" must never look like
    // success from this service's own claim's point of view -- otherwise
    // a route reclaim could never retry the handoff.
    let route = route();
    let scope = parse_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.parse", &scope)),
        complete_outcome: Some(CompleteOutcome::Completed(claim())),
        submission_result: Some(Err(503)),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let result = work_once(&port, &repository, 300);

    assert!(matches!(
        result,
        Err(ParserError::AdmissionNotAccepted(503))
    ));
    assert_eq!(
        *port.complete_calls.lock().unwrap(),
        0,
        "an unconfirmed admission must never be followed by completing this service's own claim"
    );
}

#[test]
fn a_repository_failure_never_completes_the_kernel_claim() {
    let route = route();
    let scope = parse_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.parse", &scope)),
        complete_outcome: Some(CompleteOutcome::Completed(claim())),
        ..FakePort::default()
    };
    let repository = FakeRepository {
        result: Some(Err(CandidateError::Repository)),
    };

    let result = work_once(&port, &repository, 300);

    assert!(matches!(
        result,
        Err(ParserError::Candidate(CandidateError::Repository))
    ));
    assert_eq!(*port.complete_calls.lock().unwrap(), 0);
    assert_eq!(*port.submit_calls.lock().unwrap(), 0);
}

#[test]
fn reports_fencing_loss_before_completion_without_erroring() {
    let route = route();
    let scope = parse_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.parse", &scope)),
        complete_outcome: Some(CompleteOutcome::Fenced),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(
        outcome,
        WorkOutcome::LostBeforeCompletion { route_id, claim_id }
            if route_id == "route-1" && claim_id == "claim-1"
    ));
}

#[test]
fn reports_a_lost_claim_race_without_erroring() {
    let port = FakePort {
        routes: vec![route()],
        claim_outcome: Some(ClaimOutcome::AlreadyClaimed),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(outcome, WorkOutcome::ClaimLost { route_id } if route_id == "route-1"));
}
