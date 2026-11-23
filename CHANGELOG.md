# Changelog

## 1.0.0-rc.1 — 2026-11-23 (unreleased, for owner review)

First release candidate.

### Added
* Rust workspace: shared contracts, SQLite persistence with versioned migrations, security
  primitives, connector SDK, mapping/planning/execution/validation engines, application core.
* CLI `migrate-anything` and MCP stdio server with 16 tools.
* Connectors: files (CSV/JSON/JSONL), PostgreSQL, generic REST runtime for declarative specs.
* Connector factory: OpenAPI 3.x discovery, capability manifests, spec generation, strict
  validation, contract tests, versioned registry with draft → tested → active lifecycle.
* WebAssembly extension sandbox (wasmtime) with zero host imports and resource limits.
* Operator approval workflow (TTY challenge, HMAC-signed, bound approvals).
* Durable, resumable jobs with leases, checkpoints, retries, pause/resume/cancel.
* Verification and evidence-based reports; exception archive.
* Six end-to-end demonstrations and a benchmark suite including 1,000,000 records.

### Fixed during release-candidate testing (found by the verification engine and tests)
* PostgreSQL: keyset pagination ordered by text cast of the key (skipped/repeated rows beyond
  10 pages of numeric keys).
* PostgreSQL: `char(n)` values were truncated to one character by the parameter cast.
* JSON numbers with fractions were routed through f64 (`10.10` → `10.1`); replaced by a lossless
  parser.
* Pause requested before a worker started was lost.
* Extension integrity check compared stored hashes instead of the loaded module bytes.
* Instruction-like text from API documentation was copied into generated specs; now withheld.
* Verification compared untyped destination text with typed expectations (false mismatches).
* Verification used non-indexable key lookups; id-map and verification inserts are now batched
  (about 2x execution and 10x verification throughput).
* New, empty file destinations could not be discovered or planned; a 1:1 copy mapping is proposed.
