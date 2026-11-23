# Security Policy

## Reporting a vulnerability

Please report vulnerabilities privately to the maintainers (contact to be published with the
repository). Do not open public issues for security problems. Include affected version, impact
and reproduction steps without real credentials or customer data.

## Security model (summary)

| Control | Implementation | Tested in |
|---|---|---|
| No writes without human approval | Jobs require an HMAC-SHA256-signed approval bound to plan id + SHA-256 plan hash, source/destination identity, connector versions, permitted operations, operator, target environment, expiry. Re-verified by the worker. | `demo_security.rs` |
| Agents cannot retarget or leak credentials | Over MCP an agent can only register *new* connections *without* credentials; attaching a credential or changing an existing connection requires the operator's CLI. At job start, in the worker and in verification the live system identity, connector version and environment must equal the approved plan's. | `demo_security.rs` |
| Agents cannot approve | `migrate_plan_approve` (MCP) only records a request. Approvals are created only by `plan approve` on an interactive terminal with a one-time code; non-TTY invocations are refused. | `demo_security.rs` (non-interactive CLI, MCP session) |
| Plan immutability | Plans are stored once; the hash is recomputed before approval and execution; any change invalidates approval. | `demo_security.rs` |
| Destructive operations | Listed in the plan and require `--allow-destructive` at approval. | `demo_security.rs` |
| Secrets | ChaCha20-Poly1305 with a per-workspace key, name bound as AAD, referenced by name; never accepted in connection URLs or specs; no MCP tools for secrets; redaction of errors. | `ma-security`, `demo_security.rs` |
| SSRF / network | Host allowlist per connection; DNS results checked and pinned; no redirects; link-local/metadata always denied; loopback/private only with explicit opt-in; no proxies. | `ma-security::network`, `demo_security.rs` |
| Untrusted connector specs | Strict schema (`deny_unknown_fields`), relative paths only, no traversal, only `{id}` placeholder, no DELETE, restricted auth headers; integrity hash re-checked at load. | `ma-connector-factory`, `demo_security.rs` |
| Prompt injection | Documentation is parsed as data; instruction-like passages are reported and withheld from generated specs; no document field can change policy, hosts, credentials, approvals. | `demo_unknown_api.rs`, `demo_security.rs` |
| Generated code | Only WebAssembly extensions, with zero imports, fuel/memory/deadline limits, fresh instance per call, hash-pinned in the plan; any failure aborts the job (fail closed). Never executed in the MCP server process for writes (jobs run in worker processes). | `ma-sandbox`, `demo_security.rs` |
| Audit | Every connection, generation, test, activation, approval request/grant, job start/rejection/stop and verification is recorded with the actor (`mcp-agent`, `cli:<user>`, `operator:<user>`, `worker:<pid>`). | `migrate-anything audit` |
| Local first | No record data leaves the machine; schema analysis returns statistics unless examples are explicitly requested. | design |

## Known limits

* **Same-user agents with shell access.** Key files live in the workspace directory with the
  operator's file permissions. An agent that can execute arbitrary commands *and* read files as
  the operator could read the approval key and forge an approval. The TTY challenge stops agents
  that drive tools non-interactively, but strong isolation requires running the agent under a
  different OS account (or container) than the operator and the workspace. On Unix, key files are
  created `0600`; on Windows they inherit the ACL of the user profile directory.
* The REST runtime uses rustls with the platform verifier; certificate pinning is not offered.
* Dependency auditing (`cargo audit`, `cargo deny`) runs in CI; it was not executed on the
  development machine for this release candidate.
* Exception archives and the state database contain migrated values (by design, nothing is
  silently dropped). Protect the workspace directory like the source data and apply your
  retention policy to `~/.migrate-anything`.
