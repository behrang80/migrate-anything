# Migrate Anything — Architecture

Status: v1.0 release candidate. This document describes what is implemented. Where something is
designed-for but not implemented, it says so explicitly.

## 1. Goals and non-goals

Goals: an agent-native (MCP-first) migration platform whose core is domain-neutral, that can
investigate an unfamiliar system through authorized interfaces, generate a *declarative* connector
for it, plan a migration deterministically, require human approval before writes, execute with
durable checkpoints, and prove the result with evidence-based reconciliation.

Non-goals for v1: multi-tenant SaaS, distributed workers, remote MCP transport, a browser UI,
compiling AI-generated native code.

## 2. Layering

```
 AI client (Claude Code, Cursor, ...)        Human operator (terminal)
          │  MCP / stdio                              │  CLI (TTY)
          ▼                                           ▼
   apps/mcp-server (ma-mcp)                    apps/cli (migrate-anything)
   protocol adapter only                       protocol adapter + approval UI + worker
          └──────────────────┬────────────────────────┘
                             ▼
                 crates/core (ma-core)  ← application services ("universal application core")
   ┌─────────────┬───────────┼────────────┬──────────────┬───────────────┐
   ▼             ▼           ▼            ▼              ▼               ▼
discovery /   mapping-    planning-   execution-    validation-     security
connector-    engine      engine      engine        engine          (secrets, approvals,
factory                                                              network policy)
   │                                      │
   ▼                                      ▼
connector-sdk  ◄── connectors/files, connectors/postgres, connectors/rest (declarative runtime)
   │
sandbox (wasmtime, no host imports)          persistence (SQLite, sqlx migrations)
   │
types (canonical IR, manifests, specs, plans, reports)
```

Rules enforced by the crate graph:

* `ma-types` depends on nothing internal. Every contract (records, manifests, connector specs,
  mapping specs, plans, approvals, reports) lives there so all layers share one vocabulary.
* Connectors depend only on `ma-types`, `ma-connector-sdk` and `ma-security` (for the network
  policy). They contain no migration logic and no knowledge of other systems.
* `ma-core` is the only crate that wires everything together. MCP, CLI and Studio call the same
  `ma_core::App` methods; none of them contains migration logic.
* Studio (`apps/ui-server`, crate `ma-ui`) is a third protocol adapter: an HTTP server bound to
  127.0.0.1 that serves a single-page interface (plain HTML/CSS/JavaScript embedded in the binary,
  no build step) and a JSON API over `ma_core::App`. Like the MCP server it never runs a job
  in-process; it starts the same detached worker. It acts as `Actor::Ui` and can approve only
  through a one-time code printed on the terminal it was started from (`InteractiveSession`).
* The MCP server never executes jobs in-process: `migrate_job_execute` spawns a detached worker
  process (`migrate-anything worker run --job <id>`), so a job outlives the MCP request and the
  MCP client.

## 3. Canonical data representation (`ma-types::value`, `ma-types::record`)

`Value` is a tagged enum: `Null`, `Bool`, `Int(i64)`, `Float(f64)`, `Decimal(String)`
(exact, never converted through f64), `String`, `Bytes`, `Date`, `DateTime`, `Array`,
`Object`, `Invalid { raw, reason }`, `Unsupported { type_name, raw }`.

A `Record` holds `fields: BTreeMap<String, Value>`. A *missing* field is the absence of the key,
which is distinct from `Null`, `String("")` and `Array([])`. `Invalid` and `Unsupported` carry the
raw source text so nothing is silently dropped. Each record carries `Provenance` (system, object,
source id, extraction time, source timestamps) and optional attachment references.

Data moves as bounded `RecordBatch`es (default 500 records). No component holds a whole dataset.
Columnar processing (Arrow/Polars) was not adopted for v1: the workload is record-at-a-time
field mapping, where streaming row processing is simpler. The benchmark (`benchmarks/RESULTS.md`)
measures the row transformation runtime at roughly 10× the end-to-end pipeline throughput, so
transformation is not the bottleneck; durable state writes are. A Polars comparison was not
implemented.

## 4. Connector model (`ma-connector-sdk`)

Composable traits instead of one monolith:

| Trait | Purpose |
|---|---|
| `Connector` | `identify()`, `capabilities()` (a `CapabilityManifest`) |
| `ConnectionCheck` | validate connectivity/credentials |
| `SchemaDiscovery` | return a `SystemSchema` (objects, fields, keys, relationships) |
| `RecordSource` | cursor-based pull: `read_batch(object, cursor, limit) -> Batch { records, next_cursor }` |
| `RecordSink` | `write_batch(object, records, mode) -> Vec<WriteOutcome>`, `prepare/commit` hooks |
| `RecordLookup` | find destination records by external key (used for verification and idempotency) |

The cursor model is what makes resume work: the checkpoint *is* the cursor of the last batch whose
writes were durably recorded. File cursors are byte offsets; PostgreSQL cursors are keyset
positions on the primary key; REST cursors are the API's own page token or offset.

Each connector declares a `CapabilityManifest` whose entries are tagged
`Verified | Documented | Inferred | Unsupported | Unknown`, so callers never confuse
"the docs say so" with "we tested it".

## 5. Discovery and the self-building connector factory (`ma-connector-factory`)

Pipeline for an unknown REST system:

1. Fetch the OpenAPI document (only from a URL the operator supplied; network policy applies).
2. Parse it as **data** (JSON or YAML). Descriptions are never interpreted as instructions; vendor
   extensions cannot change policy fields (see §8).
3. Build a `CapabilityManifest`: collection endpoints (`GET` returning arrays or wrapped arrays),
   create endpoints (`POST` on the same collection), item endpoints, pagination style inferred
   from parameters (`cursor`/`page_token`/`offset`/`limit`/`page`), auth schemes, rate-limit
   headers, identity fields, `$ref`-resolved field schemas and relationship hints (`*_id` fields
   matching another resource).
4. Emit a `DeclarativeConnectorSpec` (JSON, versioned, `spec_version: 1`).
5. Validate it statically: relative paths only, no host override, methods restricted per
   operation, credentials only by reference, bounded page sizes, unique resource names.
6. Contract-test it against the live system with read-only probes: connection, first page read,
   schema conformance of returned records, pagination advance without repeats, lookup by key.
   Write probes are not issued by the test suite in v1; write capability stays `Documented` until
   an approved migration writes (the first write to a destination happens only under approval).
7. Store it in the connector registry as `draft` → `tested`. Activation (`active`) requires the
   operator's TTY approval. Plans refuse to execute with non-active generated connectors.

The generic REST runtime (`connectors/rest`) is a trusted interpreter of these specs. The spec
cannot add code; it can only select among behaviours the runtime already implements.

## 6. Sandboxed extensions (`ma-sandbox`)

For transformations the mapping DSL cannot express, a WebAssembly module can be registered as an
extension. The sandbox:

* instantiates modules with **zero host imports** — a module that declares any import is rejected
  before instantiation (fail closed). Consequently it has no filesystem, network, clock, process,
  environment or credential access;
* enforces fuel (CPU), a memory cap (`StoreLimits`), and a wall-clock deadline (epoch interruption);
* exchanges data only through a tiny ABI: `alloc(len) -> ptr`, `transform(ptr, len) -> i64`
  (packed ptr/len of a JSON result);
* validates the module and runs a self-test before it can be activated.

Capabilities that WebAssembly cannot provide safely are not offered; the documented fallback is a
separate process, which v1 does not implement (it fails closed instead).

## 7. Mapping, planning, execution, validation

* **Mapping** (`ma-mapping`): `suggest()` produces `MappingSuggestion`s with confidence, evidence
  and explanation from a deterministic matcher (normalized names, synonym table, token overlap,
  type compatibility, key/relationship hints). An `AiProvider` trait allows an LLM provider to
  add suggestions; the default provider is local/heuristic and no data leaves the machine.
  Suggestions are converted into a deterministic `MappingSpec` of typed `Transform` rules
  (copy, cast, rename, enum map, date format, concat, split, default, constant, currency/decimal
  normalisation, unit conversion, reference lookup, archive, extension). Confidence below the
  review threshold marks the mapping `needs_review` and blocks plan approval until accepted.
* **Planning** (`ma-planning`): builds an immutable `MigrationPlan` (identities, connector versions,
  schemas, object mappings, dependency order, estimates, risks, verification and recovery
  strategies), canonicalises it to JSON and hashes it with SHA-256. Dependency order is a
  topological sort over reference transforms; cycles are broken by deferring the back-reference to
  a second pass (update). Dry runs read source samples, run every transform, and report failures,
  unsupported values, required-field gaps and duplicate keys without writing.
* **Execution** (`ma-execution`): per object in dependency order, a reader task prefetches batches
  into a bounded channel (backpressure); each batch is transformed, written, and then in **one
  SQLite transaction** the record outcomes, id mappings and the new checkpoint cursor are stored.
  Retries use exponential backoff with jitter on errors classified as retryable (including 429 with
  `Retry-After`). Pause/cancel are flags in the job row polled between batches. A heartbeat lease
  lets a restarted worker take over a crashed job.
* **Validation** (`ma-validation`): counts, identifier coverage (every eligible source id has an
  id-map entry), relationship integrity (every written reference resolves), sampled field
  comparison by re-reading destination records through `RecordLookup`, deterministic checksums,
  duplicate detection, and an exception summary. The report status is derived from evidence:
  `CompletedVerified` only if there are zero failures and zero unresolved discrepancies.

## 8. Security model

* **Approvals.** `migrate_plan_approve` (MCP) can only *request* approval and report status. An
  approval is created only by `migrate-anything plan approve <plan>` running on an interactive
  terminal: it refuses to run when stdin is not a TTY, prints the plan summary and hash, and
  requires typing a random challenge code. The resulting `ApprovalRecord` binds plan hash, source
  and destination identity, connector versions, permitted operations, operator identity, target
  environment and expiry, and is signed with HMAC-SHA256 using a workspace key. The executor
  re-verifies the signature and every bound field; any change to the plan changes its hash and
  invalidates the approval. Destructive operations (`truncate_destination`, `delete`) need a
  separate explicit flag in the approval.
  *Threat-model limit:* an agent with unrestricted shell access running as the same OS user can
  read the workspace key. The TTY/challenge requirement stops agents that drive tools
  non-interactively; full isolation needs OS-level separation (documented in `SECURITY.md`).
* **Secrets.** Credentials are stored encrypted (ChaCha20-Poly1305, random 96-bit nonce per
  secret) with a workspace master key, referenced by name from connections and specs, decrypted
  only inside connector construction, and redacted from logs, errors and reports.
* **Network policy.** Every outbound HTTP request goes through `NetworkPolicy`: scheme allowlist,
  per-connection host allowlist, DNS resolution with a check that resolved addresses are not
  loopback/private/link-local/metadata ranges unless the host was explicitly allowed, and
  redirects disabled. This is the SSRF defence.
* **Untrusted inputs.** OpenAPI documents, API responses and generated specs are data. Spec
  validation rejects absolute URLs, `..` segments, credential literals, unknown auth types and
  oversized limits. No field of an external document can set approval, policy or credential state.
* **Local first.** No customer data is sent to an external AI provider. External providers are
  opt-in and receive schema metadata only.

## 9. Persistence (`ma-persistence`)

SQLite (WAL mode, `synchronous=FULL`) via `sqlx`, with embedded, versioned migrations
(`crates/persistence/migrations`). Tables: workspaces, systems, connections, credentials,
connectors, connector_versions, connector_tests, schemas, extensions, plans (mapping
specifications are stored inside the immutable plan), dry_runs, approval_requests, approvals,
jobs, job_stages, checkpoints, id_mappings, record_outcomes, exceptions, validation_results,
audit_events, plus verification scratch tables. A "migration project" is represented by the
plans and jobs of a workspace in v1; there is no separate projects table. All rows carry `workspace_id`. Repository methods
take/return `ma-types` structs so a PostgreSQL implementation can be added behind the same API.

## 10. Process model

* `migrate-anything` or `migrate-anything ui` — Studio: local web interface on 127.0.0.1
  (`demo` additionally prepares a synthetic demo migration).
* `migrate-anything mcp serve` — stdio MCP server.
* `migrate-anything worker run --job <id>` — executes one job; started detached by the MCP server
  or CLI (`jobs start`). Takes a lease; exits on completion, pause or cancel.
* `migrate-anything jobs resume <id>` — clears the pause flag and starts a new worker that
  continues from the last checkpoint.

## 11. Delivery semantics

Exactly-once is **not** claimed. Per destination:

| Destination | Mechanism | Guarantee after crash |
|---|---|---|
| Files (CSV/JSONL) | output truncated back to the checkpointed byte length before resuming | no duplicates, no gaps |
| PostgreSQL | `INSERT … ON CONFLICT (external key) DO UPDATE … RETURNING`; batch in one transaction | idempotent; no duplicates |
| REST, with lookup by external key | pre-write lookup for records in an uncommitted batch | no duplicates if lookup is accurate |
| REST, no lookup (non-idempotent POST) | records of an in-flight batch are marked `requires_reconciliation` | may duplicate; reported, never hidden |
