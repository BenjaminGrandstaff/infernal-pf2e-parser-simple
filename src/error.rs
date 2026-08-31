//! Goal: give every failure mode a specific, typed variant -- matching
//! infernal-law's own error style -- rather than collapsing configuration,
//! transport, protocol, and domain failures into one opaque string.

use std::fmt::{self, Display, Formatter};

use infernal_client::ClientError;

use crate::database::DatabaseError;
use crate::domain::CandidateError;

#[derive(Debug)]
pub enum ParserError {
    MissingEnv(&'static str),
    InvalidServiceId,
    Client(ClientError),
    UnexpectedStatus(u16),
    MalformedResponse(String),
    /// `KERNEL_CA_CERT_PATH` was set but the file could not be read.
    CaCertificateUnreadable(std::io::Error),
    /// `ENROLLMENT_CHALLENGE` was set but the projected ServiceAccount
    /// token file it implies (`WORKLOAD_TOKEN_PATH`) could not be read.
    EnrollmentTokenUnreadable(std::io::Error),
    /// `ENROLLMENT_CHALLENGE` was not a valid base64url-encoded 32-byte
    /// value.
    InvalidEnrollmentChallenge,
    /// A routed request's `scope` did not decode into what
    /// `pf2e.parse` expects -- see `dispatch.rs`'s module documentation
    /// for the wire shape.
    MalformedScope(&'static str),
    /// A routed request's `action` is not one this service performs.
    UnknownAction(String),
    /// The admission Request submitted to the kernel (for
    /// `pf2e.rules.admit`) was not accepted -- see this repository's
    /// README, "Cross-service failure: admission not confirmed".
    AdmissionNotAccepted(u16),
    Database(DatabaseError),
    Candidate(CandidateError),
}

impl Display for ParserError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv(name) => {
                write!(formatter, "missing required environment variable {name}")
            }
            Self::InvalidServiceId => formatter.write_str("service ID must be a UUID"),
            Self::Client(error) => write!(formatter, "kernel client error: {error}"),
            Self::UnexpectedStatus(status) => {
                write!(formatter, "kernel returned unexpected status {status}")
            }
            Self::MalformedResponse(message) => {
                write!(formatter, "malformed kernel response: {message}")
            }
            Self::CaCertificateUnreadable(error) => {
                write!(formatter, "could not read KERNEL_CA_CERT_PATH: {error}")
            }
            Self::EnrollmentTokenUnreadable(error) => {
                write!(formatter, "could not read WORKLOAD_TOKEN_PATH: {error}")
            }
            Self::InvalidEnrollmentChallenge => {
                formatter.write_str("ENROLLMENT_CHALLENGE is not a valid base64url 32-byte value")
            }
            Self::MalformedScope(reason) => write!(formatter, "malformed request scope: {reason}"),
            Self::UnknownAction(action) => write!(formatter, "unknown action: {action}"),
            Self::AdmissionNotAccepted(status) => write!(
                formatter,
                "pf2e.rules.admit submission was not accepted (status {status})"
            ),
            Self::Database(error) => write!(formatter, "parser database error: {error}"),
            Self::Candidate(error) => write!(formatter, "parser domain error: {error}"),
        }
    }
}

impl std::error::Error for ParserError {}

impl From<ClientError> for ParserError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

impl From<DatabaseError> for ParserError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<CandidateError> for ParserError {
    fn from(error: CandidateError) -> Self {
        Self::Candidate(error)
    }
}
