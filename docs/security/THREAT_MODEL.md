# Threat model

## Assets
Source and destination data; credentials; approval authority; the integrity of migration results.

## Actors
* **Operator** — trusted human at a terminal; owns credentials and approvals.
* **AI agent** — semi-trusted: may plan and execute *approved* work; must not approve, read
  secrets, change the operator's connections, or widen what the machine reads or reaches.
* **External systems and their documentation** — untrusted input (may contain prompt injection,
  malicious schemas, hostile responses, redirects to internal addresses).
* **Generated artifacts** (specs, extensions) — untrusted until validated, tested and activated.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Agent self-approves a migration | Approval requires TTY + one-time code; MCP has no approval capability; worker re-verifies signature and binding. |
| Agent points the tool at local files or the local network | Agent-registered connections are limited to folders the operator opened (`--allow-dir`; none by default; `..` and symbolic links resolved first) and to public web APIs. Database connections and `allow_private_network` are operator-only, as are local documents outside the opened folders. |
| Agent swaps the connector of an operator's connection | An agent may attach a connection's first connector only. Replacing one is refused before anything is registered; every attachment is audited. |
| Dry run shows data from a different system than the plan | The dry run connects through the same identity check as execution and verification; a re-pointed source is refused. |
| Plan swapped after approval | Approval binds the SHA-256 of the canonical plan; plan rows are immutable and re-hashed before use. |
| Prompt injection in API docs or data | Documents parsed as data; suspicious passages reported and withheld; policies are not data-driven. |
| SSRF via generated spec or document URL | Specs cannot carry hosts; requests are built from the operator's base URL; allowlist + IP-range checks + DNS pinning + no redirects. |
| Credential exfiltration by generated code | Extensions have no imports (no syscalls/host functions); specs cannot reference secrets other than via the connection; MCP has no secret tools. |
| Resource exhaustion by extensions | Fuel, epoch deadline, memory cap, output size cap. |
| Duplicate writes after crashes | Idempotent sinks (truncate-to-checkpoint files, key upserts) or in-flight reconciliation by key lookup; otherwise records are flagged, never blindly re-sent. A lookup that cannot be answered for one record sets that record aside instead of failing the job at the same place on every resume. |
| Approval runs out while a job is paused | Nothing is written without a valid approval. When the operator approves the same plan (same hash) again, the job continues under the new approval; the change is audited. |
| Source read cut short or altered without notice | Offset- and page-numbered API listings end only on an empty page, so a server-side page cap cannot truncate a read; a repeated page is an error. CSV cells that the inferred column type would change (leading zeros, `+`, a time on a date) are kept as written. |
| Oversized API response | Responses are limited to 32 MiB while they download; documents to 20 MiB. |
| Silent data loss | Unmapped values archived; failed records preserved with full source content; report status derived from verification evidence. |
| Tampering with registered connectors/extensions | Content hashes verified at load; activation requires operator consent. |
| A web page drives Studio (cross-site request, DNS rebinding) | Studio listens on 127.0.0.1 only; every API call needs the session cookie (`HttpOnly`, `SameSite=Strict`) set from the one-time start link; the `Host` header must name 127.0.0.1/localhost on Studio's port; state-changing requests need a same-origin `Origin`; strict Content-Security-Policy and no third-party resources. |
| Another local program approves through Studio's HTTP port | Approving needs a one-time code printed on the terminal Studio was started from (5 minutes, 5 attempts, single use, bound to one plan or connector). Without an interactive terminal Studio cannot approve at all. |

## Out of scope for v1
Multi-tenant isolation; remote MCP transport (not exposed); hostile local administrator; side
channels.
