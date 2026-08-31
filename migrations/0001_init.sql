-- This is a retry cache, not the PF2e rules database -- see domain.rs's
-- own module documentation. Keyed on source identity plus parser_version
-- so a retried parse (a reclaimed pf2e.parse route, or a fresh
-- resubmission) reuses the same candidate_id(s), which is what lets
-- infernal-pf2e-rules-simple's own admission idempotency recognize a
-- retried admission instead of creating a duplicate rule.
CREATE TABLE IF NOT EXISTS parse_runs (
    document_id text NOT NULL,
    document_version bigint NOT NULL,
    content_digest bytea NOT NULL CHECK (octet_length(content_digest) = 32),
    parser_version text NOT NULL,
    request_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (document_id, document_version, content_digest, parser_version)
);

-- Backs the "source digest mismatch" check: the same identified document
-- version must have stable content.
CREATE INDEX IF NOT EXISTS parse_runs_document_version_idx
    ON parse_runs (document_id, document_version);

CREATE TABLE IF NOT EXISTS parsed_candidates (
    candidate_id text PRIMARY KEY,
    document_id text NOT NULL,
    document_version bigint NOT NULL,
    content_digest bytea NOT NULL,
    parser_version text NOT NULL,
    rule_type text NOT NULL,
    name text,
    -- Lists are stored as a JSON-array string, not `jsonb` -- see
    -- infernal-rules-extractor-pf2e's own precedent for why (avoids an
    -- extra tokio-postgres crate feature).
    traits text NOT NULL DEFAULT '[]',
    trigger text,
    requirements text NOT NULL DEFAULT '[]',
    prerequisites text NOT NULL DEFAULT '[]',
    effect text,
    related_references text NOT NULL DEFAULT '[]',
    source_text text NOT NULL,
    inferences text NOT NULL DEFAULT '[]',
    confidence double precision NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    FOREIGN KEY (document_id, document_version, content_digest, parser_version)
        REFERENCES parse_runs (document_id, document_version, content_digest, parser_version)
);

CREATE INDEX IF NOT EXISTS parsed_candidates_run_idx
    ON parsed_candidates (document_id, document_version, content_digest, parser_version);
