//! Goal: own everything about turning PF2e source text into a structured
//! *candidate* -- never an authoritative rule. This service proposes;
//! `infernal-pf2e-rules-simple` decides what becomes authoritative. See
//! `docs/architecture/decisions/0001-separate-pf2e-parsing-from-pf2e-
//! rule-authority.md` in that repository for the full reasoning.
//!
//! ## This service is not the PF2e rules database
//!
//! `CandidateRepository` exists purely to make retrying a claimed
//! `pf2e.parse` route safe and to keep `candidate_id` stable across
//! retries (see below) -- it is explicitly *not* long-term authoritative
//! storage. Nothing here tracks rule lifecycle, versions, relationships,
//! or search; that is `infernal-pf2e-rules-simple`'s job entirely.
//!
//! ## Why `candidate_id` must be stable across retries
//!
//! `infernal-pf2e-rules-simple`'s own admission idempotency is keyed on
//! `candidate_id` (see that repository's `domain.rs`). If this service
//! generated a fresh `candidate_id` on every retry of a reclaimed
//! `pf2e.parse` route, the Rules Service could never recognize a retried
//! admission as the same logical candidate, and "Parser retries after an
//! unconfirmed admission" would silently produce duplicate rules. Keying
//! this service's own local cache on `(document_id, document_version,
//! content_digest, parser_version)` -- exactly the tuple that
//! deterministically identifies "the same logical parse" -- and reusing
//! the same `candidate_id` for a repeated parse of that tuple is what
//! makes the cross-service idempotency guarantee possible at all.

use std::fmt::{self, Display, Formatter};

use uuid::Uuid;

pub const SYSTEM: &str = "pf2e";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleType {
    Action,
    Reaction,
    FreeAction,
    Feat,
    Condition,
    Unclassified,
}

impl RuleType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Reaction => "reaction",
            Self::FreeAction => "free-action",
            Self::Feat => "feat",
            Self::Condition => "condition",
            Self::Unclassified => "unclassified",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "action" => Self::Action,
            "reaction" => Self::Reaction,
            "free-action" => Self::FreeAction,
            "feat" => Self::Feat,
            "condition" => Self::Condition,
            _ => Self::Unclassified,
        }
    }
}

impl Display for RuleType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Identifies the exact authoritative source a candidate was parsed
/// from. The document itself remains owned and stored by Librarian.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReference {
    pub document_id: Uuid,
    pub document_version: i64,
    pub content_digest: [u8; 32],
    pub location: String,
    pub source_text: String,
}

/// A proposed, non-authoritative PF2e rule candidate. Everything here is
/// this service's own interpretation of `source_text`, kept explicitly
/// separate from it -- see `parser`'s own module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub candidate_id: Uuid,
    pub rule_type: RuleType,
    pub name: Option<String>,
    pub traits: Vec<String>,
    pub trigger: Option<String>,
    pub requirements: Vec<String>,
    pub effect: Option<String>,
    pub prerequisites: Vec<String>,
    pub references: Vec<String>,
    pub source_text: String,
    pub inferences: Vec<String>,
    pub confidence: f64,
}

#[derive(Clone, Debug)]
pub struct ParseOutcome {
    pub source: SourceReference,
    pub candidates: Vec<Candidate>,
    pub was_already_processed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateError {
    EmptySourceText,
    /// `(document_id, document_version)` was already parsed under a
    /// *different* `content_digest` -- the same identified document
    /// version must have stable content.
    SourceDigestMismatch,
    Repository,
}

impl Display for CandidateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySourceText => formatter.write_str("source text must not be empty"),
            Self::SourceDigestMismatch => formatter.write_str(
                "this document version was already parsed under a different content digest",
            ),
            Self::Repository => formatter.write_str("candidate repository operation failed"),
        }
    }
}

impl std::error::Error for CandidateError {}

pub trait CandidateRepository {
    /// Runs (or recognizes a prior run of) one logical parse. Safe under
    /// retry: a second call with the same `(document_id,
    /// document_version, content_digest, parser_version)` returns the
    /// same `candidate_id`(s) rather than parsing again -- see this
    /// module's own documentation for why that stability matters beyond
    /// this service's own boundary.
    fn parse_once(
        &self,
        parser_version: &str,
        source: SourceReference,
    ) -> Result<ParseOutcome, CandidateError>;
}
