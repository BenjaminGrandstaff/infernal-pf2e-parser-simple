//! Goal: implement the outbound signed calls this service makes into the
//! kernel -- both consuming governed work (ADR-0011, ILK-003/ILK-010/
//! ILK-011, the same pattern every reference service in this ecosystem
//! uses) and, uniquely among this session's reference services,
//! *submitting* a governed Request of its own (`pf2e.rules.admit`) under
//! its own identity. Every prior reference service only ever claimed and
//! completed work; this one also initiates it, because handing a parsed
//! candidate to `infernal-pf2e-rules-simple` has to travel through the
//! kernel exactly like any other cross-service communication -- see this
//! repository's README, "Service boundary".

use std::time::{SystemTime, UNIX_EPOCH};

use infernal_client::{
    CHALLENGE_LENGTH, Client, ClientCredential, EnrolledInstance, EnrollmentSubmission,
    RequestParts, SignedRequest,
};
use uuid::Uuid;

use crate::claims::{
    ClaimOutcome, ClaimRequest, CompleteOutcome, FencedActionRequest, parse_claim_response,
    parse_complete_response,
};
use crate::error::ParserError;
use crate::instance_lease::{
    RENEW_INSTANCE_PATH, RenewedLease, parse_renewal_response, renewal_request_body,
};
use crate::routed_request::{RoutedRequestOutcome, parse_routed_request_response};
use crate::routes::{ELIGIBLE_ROUTES_PATH, EligibleRoute, parse_eligible_routes};
use crate::subscriptions::{
    ACTIVE_SUBSCRIPTIONS_PATH, CreateSubscriptionRequest, SUBSCRIPTIONS_PATH, Subscription,
    parse_create_subscription_response, parse_subscription_list,
};

const SIGNATURE_VALIDITY_SECONDS: i64 = 30;
const REQUESTS_PATH: &str = "/v1/requests";
/// Sentinel schema versions for actions with no artifact content to pin
/// a real schema version to -- matches infernal-law's own
/// `no_artifact_schema_versions()` (`Uuid::from_u128(1)`/`(2)`).
const NO_ARTIFACT_SCHEMA_VERSION_ID: &str = "00000000-0000-0000-0000-000000000001";
const NO_ARTIFACT_PERMISSION_POLICY_SCHEMA_VERSION_ID: &str =
    "00000000-0000-0000-0000-000000000002";

pub trait KernelPort {
    fn eligible_routes(&self) -> Result<Vec<EligibleRoute>, ParserError>;

    fn propose_claim(
        &self,
        route_id: &str,
        lease_seconds: i64,
    ) -> Result<ClaimOutcome, ParserError>;

    fn routed_request(&self, route_id: &str) -> Result<RoutedRequestOutcome, ParserError>;

    fn complete_claim(
        &self,
        claim_id: &str,
        fencing_token: i64,
    ) -> Result<CompleteOutcome, ParserError>;

    /// Submits a new governed Request under this service's own identity.
    /// Returns `Ok(())` only on `201 Created`; any other status is
    /// reported as `ParserError::AdmissionNotAccepted` so the caller can
    /// fail the pass without completing its own claim -- see this
    /// repository's README, "Cross-service failure: admission not
    /// confirmed".
    fn submit_request(&self, action: &str, scope: &str) -> Result<(), ParserError>;
}

pub struct KernelClient {
    client: Client,
    credential: ClientCredential,
    authority: String,
}

impl KernelClient {
    pub fn new(
        credential: ClientCredential,
        authority: impl Into<String>,
    ) -> Result<Self, ParserError> {
        Ok(Self {
            client: Client::new()?,
            credential,
            authority: authority.into(),
        })
    }

    pub fn with_extra_root_certificate(
        credential: ClientCredential,
        authority: impl Into<String>,
        extra_root_certificate_pem: &[u8],
    ) -> Result<Self, ParserError> {
        Ok(Self {
            client: Client::with_extra_root_certificate(extra_root_certificate_pem)?,
            credential,
            authority: authority.into(),
        })
    }

    /// Asks the kernel to issue this workload its own enrollment challenge.
    /// Used when `ENROLLMENT_CHALLENGE` is unset, which is the normal case:
    /// a challenge is single-use, so an injected one survives only the
    /// first Pod of a Deployment revision.
    pub fn request_challenge(
        &self,
        pod_uid: &str,
        workload_token: &str,
    ) -> Result<[u8; CHALLENGE_LENGTH], ParserError> {
        let issued = self.client.request_enrollment_challenge(
            &format!("https://{}", self.authority),
            pod_uid,
            workload_token,
        )?;
        Ok(issued.challenge_bytes()?)
    }

    pub fn enroll(
        &self,
        challenge: [u8; CHALLENGE_LENGTH],
        endpoint: &str,
        pod_uid: &str,
        workload_token: String,
    ) -> Result<EnrolledInstance, ParserError> {
        let submission = EnrollmentSubmission::sign(
            &self.credential,
            challenge,
            endpoint,
            pod_uid,
            workload_token,
        )?;
        Ok(self
            .client
            .submit_enrollment(&format!("https://{}", self.authority), &submission)?)
    }

    pub fn renew_lease(&self, expected_revision: i64) -> Result<RenewedLease, ParserError> {
        let body = renewal_request_body(expected_revision);
        let signed = build_post(
            &self.credential,
            &self.authority,
            RENEW_INSTANCE_PATH,
            &body,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_renewal_response(response.status, &response.body)
    }

    pub fn active_subscriptions(&self) -> Result<Vec<Subscription>, ParserError> {
        let signed = build_get(
            &self.credential,
            &self.authority,
            ACTIVE_SUBSCRIPTIONS_PATH,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_subscription_list(response.status, &response.body)
    }

    pub fn create_subscription(&self, event_type: &str) -> Result<Subscription, ParserError> {
        let body = serde_json::to_vec(&CreateSubscriptionRequest {
            event_type: event_type.to_owned(),
        })
        .map_err(|error| ParserError::MalformedResponse(error.to_string()))?;
        let signed = build_post(
            &self.credential,
            &self.authority,
            SUBSCRIPTIONS_PATH,
            &body,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_create_subscription_response(response.status, &response.body)
    }

    pub fn ensure_subscription(&self, event_type: &str) -> Result<(), ParserError> {
        let already_active = self
            .active_subscriptions()?
            .iter()
            .any(|subscription| subscription.event_type == event_type);
        if already_active {
            return Ok(());
        }
        self.create_subscription(event_type)?;
        Ok(())
    }
}

impl KernelPort for KernelClient {
    fn eligible_routes(&self) -> Result<Vec<EligibleRoute>, ParserError> {
        let signed = build_get(
            &self.credential,
            &self.authority,
            ELIGIBLE_ROUTES_PATH,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_eligible_routes(response.status, &response.body)
    }

    fn propose_claim(
        &self,
        route_id: &str,
        lease_seconds: i64,
    ) -> Result<ClaimOutcome, ParserError> {
        let path = format!("/v1/routes/{route_id}/claims");
        let body = serde_json::to_vec(&ClaimRequest { lease_seconds })
            .map_err(|error| ParserError::MalformedResponse(error.to_string()))?;
        let signed = build_post(
            &self.credential,
            &self.authority,
            &path,
            &body,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_claim_response(response.status, &response.body)
    }

    fn routed_request(&self, route_id: &str) -> Result<RoutedRequestOutcome, ParserError> {
        let path = format!("/v1/routes/{route_id}/request");
        let signed = build_get(
            &self.credential,
            &self.authority,
            &path,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_routed_request_response(response.status, &response.body)
    }

    fn complete_claim(
        &self,
        claim_id: &str,
        fencing_token: i64,
    ) -> Result<CompleteOutcome, ParserError> {
        let path = format!("/v1/claims/{claim_id}/complete");
        let body = serde_json::to_vec(&FencedActionRequest { fencing_token })
            .map_err(|error| ParserError::MalformedResponse(error.to_string()))?;
        let signed = build_post(
            &self.credential,
            &self.authority,
            &path,
            &body,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        parse_complete_response(response.status, &response.body)
    }

    fn submit_request(&self, action: &str, scope: &str) -> Result<(), ParserError> {
        let body = serde_json::json!({
            "action": action,
            "scope": scope,
            "artifact_schema_version_id": NO_ARTIFACT_SCHEMA_VERSION_ID,
            "permission_policy_schema_version_id": NO_ARTIFACT_PERMISSION_POLICY_SCHEMA_VERSION_ID,
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|error| ParserError::MalformedResponse(error.to_string()))?;
        let signed = build_post(
            &self.credential,
            &self.authority,
            REQUESTS_PATH,
            &body_bytes,
            Uuid::new_v4(),
            unix_time(),
        )?;
        let response = self.client.send(&signed)?;
        if response.status == 201 {
            Ok(())
        } else {
            Err(ParserError::AdmissionNotAccepted(response.status))
        }
    }
}

fn build_get(
    credential: &ClientCredential,
    authority: &str,
    path: &str,
    request_id: Uuid,
    now: i64,
) -> Result<SignedRequest, ParserError> {
    let parts = RequestParts::new("GET", authority, path, "application/json", &[], request_id)?;
    sign(credential, parts, now)
}

fn build_post(
    credential: &ClientCredential,
    authority: &str,
    path: &str,
    body: &[u8],
    request_id: Uuid,
    now: i64,
) -> Result<SignedRequest, ParserError> {
    let parts = RequestParts::new(
        "POST",
        authority,
        path,
        "application/json",
        body,
        request_id,
    )?;
    sign(credential, parts, now)
}

fn sign(
    credential: &ClientCredential,
    parts: RequestParts,
    now: i64,
) -> Result<SignedRequest, ParserError> {
    let nonce = infernal_client::generate_nonce()?;
    Ok(SignedRequest::sign(
        parts,
        credential,
        now,
        now + SIGNATURE_VALIDITY_SECONDS,
        &nonce,
    )?)
}

fn unix_time() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use infernal_client::{IncomingRequest, verify_incoming};

    use super::*;

    fn incoming_from(signed: &SignedRequest) -> IncomingRequest {
        IncomingRequest::from_wire(
            signed.parts().clone(),
            &signed.service_id().to_string(),
            &signed.instance_id().to_string(),
            signed.content_digest(),
            signed.signature_input(),
            signed.signature(),
        )
        .unwrap()
    }

    #[test]
    fn the_eligible_routes_request_verifies_under_its_own_public_key() {
        let credential = ClientCredential::generate(Uuid::new_v4());

        let signed = build_get(
            &credential,
            "kernel.example.test",
            ELIGIBLE_ROUTES_PATH,
            Uuid::new_v4(),
            1_000,
        )
        .unwrap();

        let verified =
            verify_incoming(&incoming_from(&signed), credential.public_key(), 1_005).unwrap();
        assert_eq!(verified.service_id(), credential.public_key().service_id());
    }

    #[test]
    fn the_renewal_request_targets_the_instances_path() {
        let credential = ClientCredential::generate(Uuid::new_v4());
        let body = renewal_request_body(2);

        let signed = build_post(
            &credential,
            "kernel.example.test",
            RENEW_INSTANCE_PATH,
            &body,
            Uuid::new_v4(),
            1_000,
        )
        .unwrap();

        assert_eq!(signed.parts().path_and_query(), RENEW_INSTANCE_PATH);
        verify_incoming(&incoming_from(&signed), credential.public_key(), 1_005).unwrap();
    }

    #[test]
    fn the_own_request_submission_targets_v1_requests_and_carries_the_action_and_scope() {
        let credential = ClientCredential::generate(Uuid::new_v4());
        let body = serde_json::json!({
            "action": "pf2e.rules.admit",
            "scope": "abc",
            "artifact_schema_version_id": NO_ARTIFACT_SCHEMA_VERSION_ID,
            "permission_policy_schema_version_id": NO_ARTIFACT_PERMISSION_POLICY_SCHEMA_VERSION_ID,
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();

        let signed = build_post(
            &credential,
            "kernel.example.test",
            REQUESTS_PATH,
            &body_bytes,
            Uuid::new_v4(),
            1_000,
        )
        .unwrap();

        assert_eq!(signed.parts().path_and_query(), REQUESTS_PATH);
        assert!(String::from_utf8_lossy(signed.parts().body()).contains("pf2e.rules.admit"));
        verify_incoming(&incoming_from(&signed), credential.public_key(), 1_005).unwrap();
    }

    #[test]
    fn the_claim_request_targets_the_right_route_and_carries_the_lease() {
        let credential = ClientCredential::generate(Uuid::new_v4());
        let body = serde_json::to_vec(&ClaimRequest { lease_seconds: 300 }).unwrap();

        let signed = build_post(
            &credential,
            "kernel.example.test",
            "/v1/routes/route-42/claims",
            &body,
            Uuid::new_v4(),
            1_000,
        )
        .unwrap();

        assert_eq!(
            signed.parts().path_and_query(),
            "/v1/routes/route-42/claims"
        );
        assert_eq!(signed.parts().body(), br#"{"lease_seconds":300}"#);
        verify_incoming(&incoming_from(&signed), credential.public_key(), 1_005).unwrap();
    }

    #[test]
    fn the_complete_request_targets_the_right_claim_and_carries_the_fencing_token() {
        let credential = ClientCredential::generate(Uuid::new_v4());
        let body = serde_json::to_vec(&FencedActionRequest { fencing_token: 7 }).unwrap();

        let signed = build_post(
            &credential,
            "kernel.example.test",
            "/v1/claims/claim-9/complete",
            &body,
            Uuid::new_v4(),
            1_000,
        )
        .unwrap();

        assert_eq!(signed.parts().body(), br#"{"fencing_token":7}"#);
        verify_incoming(&incoming_from(&signed), credential.public_key(), 1_005).unwrap();
    }
}
