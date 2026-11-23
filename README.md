# Migrate Anything

**Any source. Any destination. Your data, liberated.**

Migrate Anything is an agent-native data migration platform. An AI client (Claude Code, Cursor,
or any MCP-compatible client) or a human at a terminal can discover how two systems work,
generate a connector for a system it has never seen, map and transform the data, get explicit
human approval, move the data with durable checkpoints, and *prove* the result with
evidence-based reconciliation.

> Status: **v1.0.0-rc.1** — release candidate for owner review. Licensing is not yet decided
> (see [License](#license)). Not published.

```
"Migrate all our data from our old system to our new system. Discover how both systems work,
 build the necessary connectors automatically, preserve relationships, custom fields and history,
 and show me the migration plan before anything is written."
```

## What works today (tested)

| Capability | Status |
|---|---|
| MCP server (stdio, 16 typed tools) and CLI over one shared Rust core | ✅ tested over a real stdio session |
| Discovery: files (CSV/JSON/JSONL), PostgreSQL catalogs, OpenAPI 3.x documents | ✅ |
| **Automatic connector generation** from OpenAPI → validated declarative spec → contract tests → registry | ✅ demonstrated on an API with no preinstalled connector |
| Capability manifests with evidence levels (verified / documented / inferred / unsupported / unknown) | ✅ |
| Heuristic semantic mapping with confidence, explanation and evidence; review gating | ✅ |
| Deterministic transformation DSL (casts, concat/split, enum maps, date parsing, exact decimal scaling, unit conversion, id-map references, nested objects, sandboxed extensions) | ✅ |
| Dependency planning incl. cycles (second-pass updates), immutable SHA-256-hashed plans, dry runs | ✅ |
| Human approval that an agent cannot grant (TTY challenge, HMAC-signed, bound to plan hash, systems, connector versions, operations, environment, expiry) | ✅ |
| Streaming execution, bounded prefetch, retries/backoff, rate limits, pause/resume/cancel, crash recovery | ✅ incl. killed worker processes |
| Verification: counts, identifier coverage, duplicates, referential integrity, sampled field comparison, full content checksum | ✅ |
| Exception archive: nothing silently dropped; failed records and unmapped values preserved | ✅ |
| WebAssembly extension sandbox (no imports, fuel, memory and deadline limits) | ✅ |
| 1,000,000-record benchmark | ✅ 1M records migrated and fully verified; ≈39k–68k records/s on a laptop SSD (I/O-bound, durable checkpoints), peak memory ≈105 MB — see [benchmarks/RESULTS.md](benchmarks/RESULTS.md) |

Built-in connectors: **files** (CSV, JSON array, JSON Lines), **PostgreSQL**, and the **generic
REST runtime** that executes generated declarative connectors. Anything else is reached by
generating a connector from an OpenAPI document. See [Known limitations](#known-limitations) for
what is *not* supported.

## Architecture

```
 AI client ──MCP/stdio──► ma-mcp ─┐                ┌─► connector-factory (OpenAPI → spec → tests)
 Operator ──terminal───► CLI ─────┼─► ma-core ─────┼─► mapping · planning · execution · validation
                                  │  (one service  ├─► connectors: files · postgres · rest runtime
          detached worker ◄───────┘   layer)       ├─► sandbox (wasmtime)   security (keys, policy)
                                                   └─► persistence (SQLite, migrations, audit)
```

* **Rust-first.** Core, CLI, MCP server, connectors and sandbox are Rust (stable 1.100).
* **MCP-first.** The MCP server is a protocol adapter; all logic lives in reusable crates.
* **Jobs outlive requests.** `migrate_job_execute` starts a detached worker process; status is in
  SQLite and survives client disconnects, restarts and crashes.
* **Declarative connectors before code.** Generated connectors are JSON specs interpreted by a
  trusted runtime; custom transforms run as import-free WebAssembly.

Details: [docs/architecture/ARCHITECTURE.md](docs/architecture/ARCHITECTURE.md).

## Install

```sh
# from source (Rust 1.100 or newer; on Windows the MSVC build tools are required)
cargo install --path apps/cli --locked
migrate-anything doctor
```

Docker: `docker build -t migrate-anything . && docker run --rm -it migrate-anything doctor`.

### Use from Claude Code (or any MCP client)

```sh
claude mcp add migrate-anything -- migrate-anything mcp serve
```

or in an MCP client configuration file:

```json
{ "mcpServers": { "migrate-anything": { "command": "migrate-anything", "args": ["mcp", "serve"] } } }
```

The agent can discover, generate connectors, plan, dry-run, request approval and run approved
jobs. **It cannot approve.** Approval happens in your terminal:

```sh
migrate-anything plan approve <plan-id>     # shows the plan, asks for a one-time code
```

## CLI quickstart

```sh
migrate-anything connections add legacy --kind files --env production \
  --settings '{"path":"examples/crm/data","format":"csv","id_fields":{"organizations":"org_id","contacts":"contact_id"}}'
migrate-anything connections add target --kind files --env test --settings '{"path":"out","format":"jsonl"}'
migrate-anything systems discover legacy
migrate-anything mapping suggest --source legacy --dest target --out mapping.json   # review & edit
migrate-anything plan create --source legacy --dest target --mapping mapping.json
migrate-anything plan preview <plan-id> --dry-run
migrate-anything plan approve <plan-id>
migrate-anything jobs start <plan-id>
migrate-anything jobs status <job-id>
migrate-anything verify <job-id>
migrate-anything report <job-id>
```

Unknown REST system:

```sh
migrate-anything credentials set fieldops-key            # stored encrypted; never exposed over MCP
migrate-anything connections add fieldops --kind rest --env production \
  --settings '{"base_url":"https://api.example.com"}' --credential-ref fieldops-key --allow-host api.example.com
migrate-anything connectors generate --connection fieldops --id fieldops-api --openapi-url https://api.example.com/openapi.json
migrate-anything connectors test --connection fieldops
migrate-anything connectors activate fieldops-api@1.0.0  # operator, needed before writing to it
```

Full walkthrough: [docs/getting-started/QUICKSTART.md](docs/getting-started/QUICKSTART.md).

## Security and approvals

* No writes without a valid, signed, unexpired operator approval bound to the exact plan hash.
  Any change to the plan invalidates it. Destructive operations need a separate flag.
* Approvals require an interactive terminal and a one-time code; piped/scripted/agent invocations
  are refused.
* Credentials are encrypted at rest (ChaCha20-Poly1305) and referenced by name; specs cannot
  contain them; MCP has no credential tools.
* Outbound HTTP: per-connection host allowlist, DNS pinning, no redirects, link-local/metadata
  addresses always denied, private networks only when explicitly allowed (SSRF protection).
* External documentation is data: instruction-like text is detected, reported and withheld.
* Local first: no record data is sent to any external AI service. Schema analysis over MCP
  returns statistics only unless examples are explicitly requested.

Threat model and limits: [SECURITY.md](SECURITY.md), [docs/security/THREAT_MODEL.md](docs/security/THREAT_MODEL.md).

## Demonstrations (automated tests)

| Demo | Test | What it proves |
|---|---|---|
| 1 CRM | `apps/cli/tests/demo_crm.rs` | CSV CRM → PostgreSQL with different schema, custom fields, enum mapping, FKs and a self-reference (second pass). 3,800 records, 5,840 relationships verified. |
| 2 Finance | `apps/cli/tests/demo_finance.rs` | Customers/invoices/lines with exact decimals → minor units, localized dates; bad records reported and preserved; audit history archived, never fabricated. |
| 3 Database | `apps/cli/tests/demo_postgres.rs` | PostgreSQL → PostgreSQL across schema differences and FKs; re-run is idempotent. |
| 4 Unknown system | `apps/cli/tests/demo_unknown_api.rs` | OpenAPI discovery → generated connector → contract tests → API→files and API→API migrations, through rate limiting. |
| 5 Interrupted | `apps/cli/tests/demo_interrupted.rs` | Worker killed between write and checkpoint, and hard-killed externally; resume yields no duplicates/gaps (files and PostgreSQL). Pause/resume/cancel. |
| 6 Security | `apps/cli/tests/demo_security.rs` | Unapproved/forged/expired/tampered approvals, non-TTY approval, malicious specs, SSRF, credential isolation, prompt injection, sandbox fail-closed, MCP session cannot approve. |

```sh
cargo test --workspace                                                   # all tests
MA_TEST_PG_URL=postgres://user@127.0.0.1:5432/postgres cargo test --workspace  # + PostgreSQL demos
cargo run -p ma-bench --release                                          # benchmarks
```

## Known limitations

* Built-in connectors: files, PostgreSQL, OpenAPI-described REST. No GraphQL, MySQL, SQL Server,
  Excel/XML, or document repositories yet; the connector model is designed for them.
* OpenAPI 2.0 (Swagger) is not supported; OAuth 2.0 flows are not implemented (use a pre-issued
  bearer token); API keys in query strings are refused by design.
* Attachments/binary content transfer is not implemented in v1 (attachment references are modeled).
* Contract tests are read-only; write capability of generated connectors stays "documented" until
  an approved migration writes.
* Mapping suggestions are heuristic and local. No external LLM provider ships in v1; the MCP
  client acts as the AI reviewer. An `AiProvider` trait exists for future providers.
* Delivery is not exactly-once for non-idempotent APIs without key lookup: interrupted in-flight
  records are flagged `requires_reconciliation` instead of re-sent.
* CSV cannot distinguish null from empty string.
* Approval protection assumes the agent cannot drive an interactive terminal as the operator; an
  agent with unrestricted shell access as the same OS user can read local key files. Use OS-level
  separation for stronger guarantees.
* Single workspace, single machine, SQLite state. PostgreSQL state backend is a planned addition.

## Roadmap

GraphQL and additional SQL dialects · OAuth 2.0 client credentials · attachment transfer ·
PostgreSQL state backend and remote authenticated MCP · optional LLM mapping providers with
metadata-only prompts · process-isolated extensions for capabilities WebAssembly cannot provide ·
web UI for plan review.

## Contributing · License

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

### License

No license has been chosen yet. Until the owner decides, all rights are reserved (see
[LICENSE](LICENSE)). The decision checklist is in
[docs/LICENSE_DECISION_CHECKLIST.md](docs/LICENSE_DECISION_CHECKLIST.md). This project is **not**
described as open source.
