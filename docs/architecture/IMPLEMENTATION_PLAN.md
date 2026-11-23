# Implementation Plan

Each phase is complete only when its acceptance test runs green. Status is tracked in
`docs/RELEASE_READINESS.md`, which records the commands run and their real results.

| Phase | Scope | Acceptance test | Status |
|---|---|---|---|
| 1 Foundation | workspace, `ma-types`, SQLite persistence + migrations, home dir, logging, CLI, MCP stdio server, worker lifecycle | `ma-persistence` (job state survives reopen, lease exclusivity); `demo_security::mcp_stdio_session_cannot_approve_or_write` (real stdio session, 16 tools); `migrate-anything doctor` | ✅ passed |
| 2 Core | canonical IR, schema inference, file connectors, PostgreSQL connector, mapping spec + runtime, dependency planning, dry run, reconciliation | `demo_crm.rs`, `demo_finance.rs`, `demo_postgres.rs` | ✅ passed |
| 3 Connector factory | OpenAPI discovery, capability manifest, spec generation + validation, contract tests, registry, approval workflow, wasm extensions | `demo_unknown_api.rs`, `ma-sandbox` tests | ✅ passed |
| 4 Reliability & security | checkpoints, lease takeover, retries/backoff, rate limits, idempotency, audit log, approval enforcement, SSRF policy, sandbox limits | `demo_interrupted.rs`, `demo_security.rs` | ✅ passed |
| 5 Performance & RC | benchmark (10k/100k/1M, relationships, concurrency, interruption), docs, CI, readiness report | `ma-bench --release`; `docs/RELEASE_READINESS.md` | ✅ local; CI/Docker not executed (no runner/Docker here) |

Order of implementation inside the repo: types → sdk → security → persistence → connectors →
mapping → planning → execution → validation → factory/sandbox → core → CLI → MCP → tests →
benchmarks → docs.

Key decisions (with rationale):

1. **Pull-based cursor reads instead of push streams.** The cursor is the checkpoint; this makes
   resume correctness local to each connector and testable.
2. **Checkpoint + outcomes + id-map in one SQLite transaction** after each destination write, so
   the job state can never claim more than was written; for files, the write itself is
   re-truncated to the checkpoint on resume.
3. **Workers are separate processes.** Crash isolation and MCP-independence come for free.
4. **Declarative specs only for generated connectors**; wasm only for pure transforms with no
   imports. No generated native code.
5. **Heuristic mapping by default**, LLM optional: deterministic and private by default.
6. **One `sqlx` dependency** for SQLite (state) and PostgreSQL (connector).
