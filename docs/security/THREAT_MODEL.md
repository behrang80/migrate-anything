# Threat model

## Assets
Source and destination data; credentials; approval authority; the integrity of migration results.

## Actors
* **Operator** — trusted human at a terminal; owns credentials and approvals.
* **AI agent** — semi-trusted: may plan and execute *approved* work; must not approve, read
  secrets or widen network access.
* **External systems and their documentation** — untrusted input (may contain prompt injection,
  malicious schemas, hostile responses, redirects to internal addresses).
* **Generated artifacts** (specs, extensions) — untrusted until validated, tested and activated.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Agent self-approves a migration | Approval requires TTY + one-time code; MCP has no approval capability; worker re-verifies signature and binding. |
| Plan swapped after approval | Approval binds the SHA-256 of the canonical plan; plan rows are immutable and re-hashed before use. |
| Prompt injection in API docs or data | Documents parsed as data; suspicious passages reported and withheld; policies are not data-driven. |
| SSRF via generated spec or document URL | Specs cannot carry hosts; requests are built from the operator's base URL; allowlist + IP-range checks + DNS pinning + no redirects. |
| Credential exfiltration by generated code | Extensions have no imports (no syscalls/host functions); specs cannot reference secrets other than via the connection; MCP has no secret tools. |
| Resource exhaustion by extensions | Fuel, epoch deadline, memory cap, output size cap. |
| Duplicate writes after crashes | Idempotent sinks (truncate-to-checkpoint files, key upserts) or in-flight reconciliation by key lookup; otherwise records are flagged, never blindly re-sent. |
| Silent data loss | Unmapped values archived; failed records preserved with full source content; report status derived from verification evidence. |
| Tampering with registered connectors/extensions | Content hashes verified at load; activation requires operator consent. |

## Out of scope for v1
Multi-tenant isolation; remote MCP transport (not exposed); hostile local administrator; side
channels.
