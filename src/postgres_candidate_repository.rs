//! Goal: persist just enough to make a retried `pf2e.parse` claim safe --
//! not to become the PF2e rules database. See `domain.rs`'s own module
//! documentation for why `candidate_id` stability across retries matters
//! beyond this service's own boundary.
//!
//! IDs are stored and queried as plain `text`, matching the convention
//! established in `infernal-rules-extractor-pf2e`.

use r2d2_postgres::postgres::Transaction;
use uuid::Uuid;

use crate::database::Database;
use crate::domain::{
    Candidate, CandidateError, CandidateRepository, ParseOutcome, RuleType, SourceReference,
};
use crate::parser;

const FIND_RUN_SQL: &str = "
    SELECT request_id FROM parse_runs
    WHERE document_id = $1 AND document_version = $2
      AND content_digest = $3 AND parser_version = $4
";

const FIND_ANY_DIGEST_FOR_VERSION_SQL: &str = "
    SELECT DISTINCT content_digest FROM parse_runs
    WHERE document_id = $1 AND document_version = $2
";

const INSERT_RUN_SQL: &str = "
    INSERT INTO parse_runs
        (document_id, document_version, content_digest, parser_version, request_id)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT (document_id, document_version, content_digest, parser_version) DO NOTHING
";

const INSERT_CANDIDATE_SQL: &str = "
    INSERT INTO parsed_candidates
        (candidate_id, document_id, document_version, content_digest, parser_version,
         rule_type, name, traits, trigger, requirements, prerequisites, effect,
         related_references, source_text, inferences, confidence)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
";

const SELECT_CANDIDATES_SQL: &str = "
    SELECT candidate_id, rule_type, name, traits, trigger, requirements, prerequisites,
           effect, related_references, source_text, inferences, confidence
    FROM parsed_candidates
    WHERE document_id = $1 AND document_version = $2
      AND content_digest = $3 AND parser_version = $4
";

pub struct PostgresCandidateRepository {
    database: Database,
}

impl PostgresCandidateRepository {
    pub const fn new(database: Database) -> Self {
        Self { database }
    }

    pub const fn database(&self) -> &Database {
        &self.database
    }
}

fn repository_error<E: std::fmt::Display>(error: E) -> CandidateError {
    eprintln!("candidate repository error: {error}");
    CandidateError::Repository
}

fn encode_string_list(values: &[String]) -> String {
    serde_json::to_string(values).expect("a string list always serializes to JSON")
}

fn decode_string_list(value: &str) -> Vec<String> {
    serde_json::from_str(value).unwrap_or_default()
}

fn load_candidates(
    transaction: &mut Transaction<'_>,
    document_id: &str,
    document_version: i64,
    digest: &[u8],
    parser_version: &str,
) -> Result<Vec<Candidate>, CandidateError> {
    let rows = transaction
        .query(
            SELECT_CANDIDATES_SQL,
            &[&document_id, &document_version, &digest, &parser_version],
        )
        .map_err(repository_error)?;
    rows.iter()
        .map(|row| {
            let candidate_id: String = row.get(0);
            let rule_type: String = row.get(1);
            let traits: String = row.get(3);
            let requirements: String = row.get(5);
            let prerequisites: String = row.get(6);
            let related_references: String = row.get(8);
            let inferences: String = row.get(10);
            Ok(Candidate {
                candidate_id: candidate_id
                    .parse()
                    .map_err(|_| CandidateError::Repository)?,
                rule_type: RuleType::parse(&rule_type),
                name: row.get(2),
                traits: decode_string_list(&traits),
                trigger: row.get(4),
                requirements: decode_string_list(&requirements),
                prerequisites: decode_string_list(&prerequisites),
                effect: row.get(7),
                references: decode_string_list(&related_references),
                source_text: row.get(9),
                inferences: decode_string_list(&inferences),
                confidence: row.get(11),
            })
        })
        .collect()
}

impl CandidateRepository for PostgresCandidateRepository {
    fn parse_once(
        &self,
        parser_version: &str,
        source: SourceReference,
    ) -> Result<ParseOutcome, CandidateError> {
        if source.source_text.trim().is_empty() {
            return Err(CandidateError::EmptySourceText);
        }

        let document_id = source.document_id.to_string();
        let digest = source.content_digest.to_vec();

        let mut connection = self.database.connection().map_err(repository_error)?;
        let mut transaction = connection.transaction().map_err(repository_error)?;

        let existing_digests = transaction
            .query(
                FIND_ANY_DIGEST_FOR_VERSION_SQL,
                &[&document_id, &source.document_version],
            )
            .map_err(repository_error)?;
        for row in &existing_digests {
            let existing: Vec<u8> = row.get(0);
            if existing != digest {
                return Err(CandidateError::SourceDigestMismatch);
            }
        }

        let already_ran = transaction
            .query_opt(
                FIND_RUN_SQL,
                &[
                    &document_id,
                    &source.document_version,
                    &digest,
                    &parser_version,
                ],
            )
            .map_err(repository_error)?
            .is_some();

        if already_ran {
            let candidates = load_candidates(
                &mut transaction,
                &document_id,
                source.document_version,
                &digest,
                parser_version,
            )?;
            transaction.commit().map_err(repository_error)?;
            return Ok(ParseOutcome {
                source,
                candidates,
                was_already_processed: true,
            });
        }

        let candidates = parser::parse_candidates(&source.source_text);
        let request_id_text = Uuid::new_v4().to_string();

        let inserted = transaction
            .execute(
                INSERT_RUN_SQL,
                &[
                    &document_id,
                    &source.document_version,
                    &digest,
                    &parser_version,
                    &request_id_text,
                ],
            )
            .map_err(repository_error)?;

        if inserted == 0 {
            // Lost a race against a concurrent identical parse.
            let candidates = load_candidates(
                &mut transaction,
                &document_id,
                source.document_version,
                &digest,
                parser_version,
            )?;
            transaction.commit().map_err(repository_error)?;
            return Ok(ParseOutcome {
                source,
                candidates,
                was_already_processed: true,
            });
        }

        for candidate in &candidates {
            let candidate_id = candidate.candidate_id.to_string();
            let traits = encode_string_list(&candidate.traits);
            let requirements = encode_string_list(&candidate.requirements);
            let prerequisites = encode_string_list(&candidate.prerequisites);
            let references = encode_string_list(&candidate.references);
            let inferences = encode_string_list(&candidate.inferences);
            transaction
                .execute(
                    INSERT_CANDIDATE_SQL,
                    &[
                        &candidate_id,
                        &document_id,
                        &source.document_version,
                        &digest,
                        &parser_version,
                        &candidate.rule_type.as_str(),
                        &candidate.name,
                        &traits,
                        &candidate.trigger,
                        &requirements,
                        &prerequisites,
                        &candidate.effect,
                        &references,
                        &candidate.source_text,
                        &inferences,
                        &candidate.confidence,
                    ],
                )
                .map_err(repository_error)?;
        }

        transaction.commit().map_err(repository_error)?;

        Ok(ParseOutcome {
            source,
            candidates,
            was_already_processed: false,
        })
    }
}
