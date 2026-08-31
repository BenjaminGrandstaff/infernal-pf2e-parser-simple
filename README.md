# infernal-pf2e-parser-simple

> The PF2e Parser transforms Pathfinder 2e source content into structured
> rule candidates. It does not own authoritative rule data.

> All communication with other services is mediated through Infernal
> Law.

This is a domain service running on top of the
[infernal-law](https://github.com/BenjaminGrandstaff/infernal-law)
kernel. It is one half of a split originally implemented as a single
service, `infernal-rules-extractor-pf2e`; see
[`infernal-pf2e-rules-simple`'s ADR-0001](https://github.com/BenjaminGrandstaff/infernal-pf2e-rules-simple/blob/main/docs/architecture/decisions/0001-separate-pf2e-parsing-from-pf2e-rule-authority.md)
for why they were split, and
[`minimum-viable-kernel.md`](https://github.com/BenjaminGrandstaff/infernal-law/blob/main/docs/architecture/minimum-viable-kernel.md)
for the architectural authority both services were built against.

## Architecture

```text
Librarian
    |
    | pf2e.parse
    v
Infernal Law
    |
    v
PF2e Parser
    |
    | pf2e.rules.admit
    v
Infernal Law
    |
    v
PF2e Rules Service
```

This service owns:

- PF2e text parsing
- extraction logic
- normalization logic
- identifying rule type
- extracting names, traits, triggers, requirements, effects,
  prerequisites, and explicit references
- recording parser confidence
- preserving source provenance
- distinguishing explicit source facts from parser inference

This service does **not** own:

- authoritative PF2e rules
- the PF2e rule database
- permanent rule IDs
- rule lifecycle
- authoritative rule versions
- rule search
- rule indexes
- graph storage
- long-term relationship storage
- canonical deduplication decisions
- rule supersession policy

All of those belong to `infernal-pf2e-rules-simple`. This service never
calls it directly, shares a database with it, or shares filesystem
state with it -- every candidate this service produces reaches the
Rules Service only through a governed `pf2e.rules.admit` Infernal Law
Request, submitted under this service's own identity.

## Kernel payload limitations

infernal-law's MVP `Request` carries only a namespaced `action`, a
`scope` string bounded to 200 characters, and schema version references
-- ILK-006 artifact/content mediation is Future Kernel, not built. This
service inherits the same constraint `infernal-rules-extractor-pf2e`
documented first (see that repository's own README for the original
accounting), now on *both* of its governed hops:

- **input** (`pf2e.parse`, received): `scope` is
  ```text
  <document_id>@<document_version>#<location>!<content_digest_b64url>|<source_text>
  ```
- **output** (`pf2e.rules.admit`, submitted): `scope` is
  ```text
  <candidate_id>@<parser_version>#<rule_type>!<confidence>|<document_id>~<document_version>~<digest_b64>~<location>~<name>
  ```

The admit scope carries the candidate's identity, classification,
confidence, and full source provenance (a complete 32-byte content
digest, for exact provenance) -- but **not** the candidate's normalized
fields (`trigger`, `requirements`, `prerequisites`, `effect`,
`references`). There is no room left in 200 characters once exact
provenance is included alongside candidate identity. This is not routed
around with a side channel to the Rules Service: it is the same ILK-006
gap already documented for Librarian and the original single-service
extractor, now shown to bind on a second, independent hop. A real
deployment needs a real kernel-mediated content channel before either
hop can carry realistic-length PF2e content.

## First milestone

Given one Pathfinder 2e source document fragment, produce zero or more
structured rule candidates and submit each one for admission. This
milestone supports five initial categories (`action`, `reaction`,
`free-action`, `feat`, `condition`) plus an explicit `unclassified`
category for anything that does not fit clearly, rather than inventing
semantics for content the source does not support.

## Parsing behavior

`src/parser.rs` keeps four things distinct on every parse:

- **source fact** -- `source_text`, the block exactly as received;
- **normalized representation** -- `name`, `traits`, `trigger`,
  `requirements`, `prerequisites`, `effect`, populated *only* from an
  explicitly labeled line;
- **parser inference** -- `inferences`, anything concluded but not
  explicitly labeled, kept out of every normalized field;
- **uncertainty** -- `confidence`, lowered whenever structure was
  incomplete or unrecognized, never inflated.

Content this parser cannot make sense of becomes a single low-confidence
`unclassified` candidate wrapping the whole block, not an error --
"malformed" PF2e content is a low-confidence parse, not a failure.
`Requirements` (conditions that must hold to *use* an action or
reaction) and `Prerequisites` (what a *feat* requires to be taken) are
kept as separate fields, matching the candidate shape below.

## Candidate shape

```json
{
  "candidate_id": "...",
  "system": "pf2e",
  "rule_type": "reaction",
  "name": "Reactive Strike",
  "source_text": "...",
  "parsed": {
    "traits": [],
    "trigger": "...",
    "requirements": [],
    "effect": "...",
    "prerequisites": [],
    "references": []
  },
  "source": {
    "document_id": "...",
    "document_version": "...",
    "content_digest": "...",
    "location": "..."
  },
  "parser": {
    "parser_version": "pf2e-parser-0.1.0",
    "confidence": 0.9
  }
}
```

This is `domain::Candidate`'s shape; the wire format actually
transmitted to the Rules Service is the narrower scope string described
above, not this full JSON -- see "Kernel payload limitations".

## Cross-service failure: admission not confirmed

`work_once` (`src/lib.rs`) parses and submits the `pf2e.rules.admit`
Request(s) *before* completing this service's own `pf2e.parse` claim,
and only completes that claim if every submission was accepted (kernel
status `201`). If a submission is not confirmed, this pass fails outright
without completing the claim -- the route can be reclaimed and retried.
On retry, `postgres_candidate_repository`'s own idempotency (keyed on
`(document_id, document_version, content_digest, parser_version)`, see
`domain.rs`) reproduces the *same* `candidate_id`, which is what lets
the Rules Service's own admission idempotency recognize the retried
submission and avoid creating a duplicate rule. Proven directly:
`an_unconfirmed_admission_never_completes_this_services_own_claim` in
`tests/kernel_adapter.rs`, and
`repeated_parsing_of_the_same_source_and_parser_version_reuses_the_same_candidate_id`
in `tests/domain_repository.rs`.

### Everything else tested

- **Parser crash before extraction / after extraction but before local
  commit** -- parsing and its commit happen inside one database
  transaction; there is no window between them.
- **Parser restart** -- a fresh `Database`/`PostgresCandidateRepository`
  reconnecting reproduces the same cached candidates for a repeated
  source (`a_new_parse_persists_candidates_with_a_stable_id`).
- **Duplicate rule candidate / stale-fenced worker** -- a fenced claim is
  reported as `LostBeforeCompletion`, never `Completed`
  (`reports_fencing_loss_before_completion_without_erroring`); the
  candidate committed locally beforehand is harmless because the Rules
  Service's own idempotency recognizes it on the next legitimate
  completion.
- **Missing source provenance / malformed scope** --
  `a_malformed_scope_is_rejected_before_touching_the_repository_or_kernel`.
- **Source digest mismatch** --
  `the_same_document_version_with_a_different_digest_is_rejected_as_a_mismatch`
  in `tests/domain_repository.rs`.
- **Database unavailable** -- a repository failure never reaches
  `submit_request` or `complete_claim`
  (`a_repository_failure_never_completes_the_kernel_claim`).
- **Kernel unavailable** -- `work_once` returns `Err`, logged and
  retried on the next poll tick.

## Infernal Law integration

Using [`infernal-client-rs`](https://github.com/BenjaminGrandstaff/infernal-client-rs),
this service:

1. enrolls as its own service principal (ADR-0008);
2. renews its own instance lease proactively via
   `POST /v1/instances/renew`, well before the kernel's default
   60-second grant expires -- included from the start, not retrofitted
   (this route exists because `infernal-librarian-simple`'s own live
   testing found that no polling client could keep working past 60
   seconds without it);
3. maintains an active inclusive subscription for `pf2e.parse`;
4. polls `GET /v1/routes/eligible`;
5. claims eligible work under its own authenticated identity;
6. reads the routed Request;
7. parses it into candidates;
8. persists enough local state for safe retry;
9. submits `pf2e.rules.admit` for each candidate, under its own
   identity -- the one capability no other reference service in this
   ecosystem has needed before: initiating governed work, not just
   consuming it;
10. completes its own claim only once every submission is confirmed.

### Worker ownership and fencing

This service claims and completes its own `pf2e.parse` work directly --
there is no delegation. If this service's claim is fenced before it can
complete (having already submitted admission Requests), it reports the
loss rather than a false success; the already-submitted admissions are
unaffected, since they travel through the kernel and Rules Service
independently of whether this service's own upstream claim gets marked
complete.

## Idempotency

Parser idempotency: the same `(source_document_id,
source_document_version, source_content_digest, parser_version)`
produces the same logical result -- specifically, the same
`candidate_id`(s), never a fresh set on retry. This is what makes the
Rules Service's own, separate admission idempotency effective across a
retry; see "Cross-service failure: admission not confirmed" above.

## What this service must not become

- not a generic document parser
- not a universal rules engine
- not an Infernal Law kernel module
- not a Pathfinder character builder
- not a Pathfinder rules database
- not a search engine
- not a direct Librarian client
- not an AI agent orchestration platform

## Test corpus

`tests/domain_repository.rs` and the unit tests in `src/parser.rs`
exercise a small, original, hand-written set of PF2e-style fixtures --
not scraped or bulk-ingested from any published rulebook -- covering a
simple action, a reaction with a trigger, a feat with prerequisites, a
condition, an explicit cross-reference, and ambiguous or partially
labeled structure.

## Configuration

- `KERNEL_AUTHORITY` (required)
- `KERNEL_CA_CERT_PATH` (optional)
- `PARSER_SERVICE_ID` (required) -- already provisioned and enrolled
  with the kernel.
- `CLAIM_LEASE_SECONDS` (default `300`)
- `POLL_INTERVAL_SECONDS` (default `5`)
- `ENROLLMENT_CHALLENGE` (optional) -- when set, `SERVICE_ENDPOINT` and
  `POD_UID` become required, and `WORKLOAD_TOKEN_PATH` (default
  `/var/run/secrets/infernal-law-enrollment/token`) must point at this
  Pod's own projected token. Unset skips enrollment and lease renewal
  both.
- `PF2E_PARSER_DATABASE_URL` (required) -- this service's own retry
  cache, entirely separate from infernal-law's own database and from
  `infernal-pf2e-rules-simple`'s own database.
- `HEALTH_ADDRESS` (default `0.0.0.0:8090`)

## Development

```sh
cargo build
cargo test
```

## Tests

- **Unit tests** (`cargo test --lib`) -- wire-format parsing, signature
  construction, and `src/parser.rs`'s PF2e-specific parsing rules.
- **Domain tests** (`tests/domain_repository.rs`, live PostgreSQL,
  `#[ignore]`d):
  ```sh
  export PF2E_PARSER_DATABASE_URL='postgres://...'
  cargo test --test domain_repository -- --ignored --test-threads=1
  ```
- **Kernel adapter tests** (`tests/kernel_adapter.rs`) -- `work_once`'s
  orchestration against fakes for both the kernel and the candidate
  repository, including the critical cross-service failure test.

## Podman

```sh
podman build -t localhost/infernal-pf2e-parser-simple:latest .
```

## Kubernetes

`k8s/base/` deploys this service the same way every other reference
service in this ecosystem is deployed. Before it can do anything, an
operator must provision, out of band:

1. an `identities` row for `PARSER_SERVICE_ID`;
2. an enrollment binding for this service's Kubernetes ServiceAccount,
   enabled;
3. `service_communication_admission` enabled for that identity;
4. an ILK-002 authority grant for `subscription.create` under this
   service's own identity;
5. an ILK-002 authority grant for `pf2e.rules.admit` under this
   service's own identity -- required because this service *submits*
   that Request itself, not just `pf2e.parse` consumption;
6. a real ADR-0008 enrollment challenge, set as `ENROLLMENT_CHALLENGE`.

## Scope discipline

Before proposing a change to `infernal-law` on this project's behalf,
stop and ask whether it protects authority, communication, or
correctness. If not, it belongs in this service or in
`infernal-pf2e-rules-simple`. Nothing in this repository's development
required a kernel change. The one gap this service's own split surfaced
-- no kernel-mediated way to carry a candidate's full normalized content
alongside exact provenance in 200 characters -- is documented above
("Kernel payload limitations"), not routed around with a direct call to
the Rules Service.

## Success criterion

PF2e parsing can be replaced, upgraded, or run independently without
changing the authoritative PF2e rules database, and neither this
service nor the Rules Service directly knows how to communicate with
the other except through Infernal Law's governed contracts.

## License

MIT. See [LICENSE](LICENSE).
