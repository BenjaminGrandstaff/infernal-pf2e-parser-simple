//! Goal: prove the full two-hop vertical slice against real deployed
//! infrastructure -- a Requester submits a real signed `pf2e.parse`
//! Request, a real running `infernal-pf2e-parser-simple` Deployment
//! claims it, parses it, and submits `pf2e.rules.admit` to the kernel,
//! and a real running `infernal-pf2e-rules-simple` Deployment claims and
//! admits it. This test only drives the Requester side; both PF2e
//! services do their own real work independently, via their own poll
//! loops, exactly as they would in production.
//!
//! Requires a real deployed kernel, both PF2e services running, and a
//! separate enrolled Requester identity with a grant for `pf2e.parse`:
//! `KERNEL_AUTHORITY`, `KERNEL_CA_CERT_PATH`, `REQUESTER_SERVICE_ID`,
//! `REQUESTER_ENROLLMENT_CHALLENGE`, `REQUESTER_SERVICE_ENDPOINT`,
//! `REQUESTER_POD_UID`, `REQUESTER_WORKLOAD_TOKEN_PATH`.

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use infernal_client::{
    Client, ClientCredential, EnrollmentSubmission, RequestParts, SignedRequest,
};
use uuid::Uuid;

fn unix_time() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

#[test]
#[ignore = "requires a real deployed infernal-law kernel and both real running PF2e Deployments -- see this file's own module documentation"]
fn a_real_requester_drives_parse_and_admission_through_live_deployments() {
    let authority = env::var("KERNEL_AUTHORITY").unwrap();
    let service_id: Uuid = env::var("REQUESTER_SERVICE_ID").unwrap().parse().unwrap();
    let credential = ClientCredential::generate(service_id);
    let http = match env::var("KERNEL_CA_CERT_PATH") {
        Ok(path) => Client::with_extra_root_certificate(&std::fs::read(&path).unwrap()).unwrap(),
        Err(_) => Client::new().unwrap(),
    };

    let challenge_b64 = env::var("REQUESTER_ENROLLMENT_CHALLENGE").unwrap();
    let endpoint = env::var("REQUESTER_SERVICE_ENDPOINT").unwrap();
    let pod_uid = env::var("REQUESTER_POD_UID").unwrap();
    let token_path = env::var("REQUESTER_WORKLOAD_TOKEN_PATH").unwrap();
    let workload_token = std::fs::read_to_string(&token_path)
        .unwrap()
        .trim()
        .to_owned();
    let challenge: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&challenge_b64)
        .unwrap()
        .try_into()
        .unwrap();
    let submission =
        EnrollmentSubmission::sign(&credential, challenge, &endpoint, &pod_uid, workload_token)
            .unwrap();
    let enrolled = http
        .submit_enrollment(&format!("https://{authority}"), &submission)
        .unwrap();
    println!("requester enrolled: {enrolled:?}");

    let document_id = Uuid::new_v4();
    let unique = Uuid::new_v4().simple().to_string();
    let source_text = format!("Action\nName: LiveSlice{unique}\nEffect: Move.");
    let scope = format!(
        "{document_id}@1#p1!{}|{source_text}",
        URL_SAFE_NO_PAD.encode([0_u8; 32])
    );

    let body = serde_json::json!({
        "action": "pf2e.parse",
        "scope": scope,
        "artifact_schema_version_id": "00000000-0000-0000-0000-000000000001",
        "permission_policy_schema_version_id": "00000000-0000-0000-0000-000000000002",
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let request_id = Uuid::new_v4();
    let now = unix_time();
    let parts = RequestParts::new(
        "POST",
        &authority,
        "/v1/requests",
        "application/json",
        &body_bytes,
        request_id,
    )
    .unwrap();
    let nonce = infernal_client::generate_nonce().unwrap();
    let signed = SignedRequest::sign(parts, &credential, now, now + 30, &nonce).unwrap();
    let response = http.send(&signed).unwrap();
    assert_eq!(
        response.status,
        201,
        "pf2e.parse submission must be accepted: {:?}",
        String::from_utf8_lossy(&response.body)
    );
    println!("submitted pf2e.parse request_id={request_id} scope={scope:?}");
}
