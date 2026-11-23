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

### Added after the first candidate build
* **Migrate Anything Studio**, a local web interface (`migrate-anything` without arguments, or
  `migrate-anything ui`): five guided steps (connect, map, plan, approve, run & proof), a mapping
  board with wires between fields, a live run view where code rain follows the records being
  migrated, pause/resume/cancel, automatic verification and the report. `migrate-anything demo`
  opens it with a ready-made demo of 90,000 synthetic records. Finished runs can be replayed.
* Approval from Studio through a one-time code shown on the terminal it was started from; the
  HTTP server is local-only with session token, Host and Origin checks and a strict CSP.
* `studio.cmd` / `studio.sh` launchers that build on first start and open Studio.
* Mapping suggestions: a destination's required primary key now receives the source record ID
  (as a guess to confirm), names that differ by a qualifier are matched (`org_name` → `name`),
  and the source ID is no longer guessed into unrelated fields.
* Studio end-to-end tests (`apps/cli/tests/studio_ui.rs`).

### Changed
* Running `migrate-anything` without a command opens Studio instead of printing help.
* A relative folder given for a file connection is stored as an absolute path, so the worker
  process finds it regardless of where it is started.
* A job whose worker cannot start is marked failed with the reason instead of staying pending.

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

### Fixed after code review
* Security: an approved job followed a connection that had been re-pointed after approval. Jobs,
  workers and verification now refuse when the live system, connector or environment differs
  from the approved plan.
* Security: an agent could define a connection that reused a stored credential for a host of its
  choosing. Over MCP, agents can now only register new, credential-less connections; attaching
  credentials or changing an existing connection is an operator (CLI) action.
* File destinations (CSV/JSON) could not be resumed when a job stopped before its first
  checkpoint.
* Text such as `+46701112233` was typed as an integer and lost its plus sign.
* Non-idempotent REST creates were re-sent after 5xx or post-send transport errors.
* The worker lease is now renewed by a heartbeat, independent of batch duration.
* Verification compared numeric-looking text fields numerically (`1.50` = `1.5`).
* OpenAPI downloads are capped at 20 MiB while streaming.
* Redaction helper could panic on multi-byte text and was not applied to MCP errors.
* A stale error message survived a successful resume.
