# Migrate Anything 1.0.0-rc.1 — release notes (draft, not published)

The first release candidate of a Rust, MCP-first, local-first universal migration platform.

## Highlights
* **Studio.** Run `migrate-anything` and a local web interface opens: connect two systems, check
  the mapping on a board, read the plan, approve with a code from your terminal, and watch the
  records arrive as code rain. `migrate-anything demo` shows it with 90,000 synthetic records.
* **Talk to it from your AI client.** 16 MCP tools cover discovery, connector generation and
  testing, schema analysis, mapping suggestions, plans and dry runs, approval requests, job
  control, verification and reports.
* **Self-building connectors.** Point it at an OpenAPI document; it generates a validated,
  versioned declarative connector, contract-tests it read-only and stores it for reuse.
* **You approve, not the AI.** Writes require an operator approval made on a real terminal,
  cryptographically bound to the exact plan.
* **Proof, not promises.** Every job can be verified: key coverage, duplicates, referential
  integrity, sampled field comparison and a full content checksum. Reports state only what the
  evidence supports.
* **Nothing silently lost.** Unmapped values and failed records are archived in full.
* **Crash-safe.** Durable per-batch checkpoints; killed workers resume without duplicates.

## Supported in this release
Files (CSV/JSON/JSONL), PostgreSQL, OpenAPI 3.x REST systems (API key, bearer, basic auth;
cursor/offset/page pagination). Windows verified locally; Linux/macOS builds are configured in CI
but were not executed for this candidate.

## Not yet supported
GraphQL, other SQL databases, OAuth 2.0 flows, OpenAPI 2.0, attachment transfer, external LLM
mapping providers, remote MCP. See README "Known limitations".

## Upgrade notes
First release; state schema version 1.
