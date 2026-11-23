# Security Policy

## Reporting a vulnerability

Please report vulnerabilities privately, not in public issues. On the repository's GitHub page
open **Security → Report a vulnerability**: the report is visible only to you and the
maintainers. If that button is not shown, open an issue that says only that you have a security
report, without any details, and a maintainer will arrange a private channel. Include affected
version, impact and reproduction steps without real credentials or customer data.

## Security model (summary)

| Control | Implementation | Tested in |
|---|---|---|
| No writes without human approval | Jobs require an HMAC-SHA256-signed approval bound to plan id + SHA-256 plan hash, source/destination identity, connector versions, permitted operations, operator, target environment, expiry. Re-verified by the worker. | `demo_security.rs` |
| Agents cannot retarget or leak credentials | Over MCP an agent can only register *new* connections *without* credentials; attaching a credential or changing an existing connection requires the operator's CLI. At job start, in the worker and in verification the live system identity, connector version and environment must equal the approved plan's. | `demo_security.rs` |
| Agents cannot widen what the tool reads or reaches | A connection an agent registers by itself can be a folder inside one the operator opened (`mcp serve --allow-dir`, `MA_AGENT_DIRS`; none by default; paths are resolved through `..` and symbolic links before the check) or a public web API. Database connections, `allow_private_network` and local documents outside the opened folders are refused. An agent can give a connection its first connector but cannot replace the one it uses. | `demo_security.rs` (in-process and over a real MCP session) |
| Agents cannot approve | `migrate_plan_approve` (MCP) only records a request. Approvals are created only by `plan approve` on an interactive terminal with a one-time code; non-TTY invocations are refused. | `demo_security.rs` (non-interactive CLI, MCP session) |
| Approving in Studio | Studio (the local web interface) can approve only when it was started from an interactive terminal. Approving prints a one-time 6-character code **in that terminal**; the browser has to send it back. The code is valid for 5 minutes and 5 attempts, works once, and is bound to one plan (or one connector activation). Something that can reach the HTTP port but cannot read the terminal cannot approve. Started without a terminal, Studio offers no approval at all. | `studio_ui.rs` |
| Studio's HTTP surface | Listens on 127.0.0.1 only. A random 192-bit session token is delivered in the start link and kept in an `HttpOnly`, `SameSite=Strict` cookie; every API call requires it. The `Host` header must be `127.0.0.1` or `localhost` on Studio's port (DNS rebinding), and every state-changing request must carry a same-origin `Origin` (cross-site requests). Strict Content-Security-Policy; the page loads no third-party scripts, fonts or other resources. | `studio_ui.rs` |
| Plan immutability | Plans are stored once; the hash is recomputed before approval and execution; any change invalidates approval. | `demo_security.rs` |
| Destructive operations | Listed in the plan and require `--allow-destructive` at approval. | `demo_security.rs` |
| Secrets | ChaCha20-Poly1305 with a per-workspace key, name bound as AAD, referenced by name; never accepted in connection URLs or specs; no MCP tools for secrets; redaction of errors. | `ma-security`, `demo_security.rs` |
| SSRF / network | Host allowlist per connection; DNS results checked and pinned; no redirects; link-local/metadata always denied; loopback/private only with explicit opt-in; no proxies. | `ma-security::network`, `demo_security.rs` |
| Untrusted connector specs | Strict schema (`deny_unknown_fields`), relative paths only, no traversal, only `{id}` placeholder, no DELETE, restricted auth headers; integrity hash re-checked at load. | `ma-connector-factory`, `demo_security.rs` |
| Prompt injection | Documentation is parsed as data; instruction-like passages are reported and withheld from generated specs; no document field can change policy, hosts, credentials, approvals. | `demo_unknown_api.rs`, `demo_security.rs` |
| Generated code | Only WebAssembly extensions, with zero imports, fuel/memory/deadline limits, fresh instance per call, hash-pinned in the plan; any failure aborts the job (fail closed). Never executed in the MCP server process for writes (jobs run in worker processes). | `ma-sandbox`, `demo_security.rs` |
| Audit | Every connection, generation, test, activation, approval request/grant, job start/rejection/stop and verification is recorded with the actor (`mcp-agent`, `cli:<user>`, `ui:<user>`, `operator:<user>`, `worker:<pid>`). | `migrate-anything audit` |
| Local first | No record data leaves the machine; schema analysis returns statistics unless examples are explicitly requested. | design |

## Known limits

* **Same-user agents with shell access.** Key files live in the workspace directory with the
  operator's file permissions. An agent that can execute arbitrary commands *and* read files as
  the operator could read the approval key and forge an approval. The TTY challenge stops agents
  that drive tools non-interactively, but strong isolation requires running the agent under a
  different OS account (or container) than the operator and the workspace. On Unix, key files are
  created `0600`; on Windows they inherit the ACL of the user profile directory.
* **Studio and same-user processes.** The start link (with the session token) and the approval
  codes are printed to the terminal Studio was started from. A process that can read that
  terminal's output as the same OS user is inside the same boundary as above. The token is
  valid only while that Studio process runs; a new one is created at every start.
* The REST runtime uses rustls with the platform verifier; certificate pinning is not offered.
* Dependency auditing runs in CI. `cargo deny` (licenses, banned crates, sources) was also run
  locally and passes; `cargo audit` was not run locally. The CI run of 2026-11-23 reported one
  advisory as a warning: `chrono` is flagged as unmaintained (RUSTSEC-2026-0543), with no fixed
  version available.
* **An agent can read what you open to it.** Folders given with `--allow-dir` and every
  connection you add can be profiled by the agent, including example values when it asks for
  them. Open only what the migration needs.
* Exception archives and the state database contain migrated values (by design, nothing is
  silently dropped). Protect the workspace directory like the source data and apply your
  retention policy to `~/.migrate-anything`.
