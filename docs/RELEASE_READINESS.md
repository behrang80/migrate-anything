# Release readiness report — 1.0.0-rc.1

Date: 2026-11-23 · Prepared for owner review · Pushed to a private GitHub repository; **no
release published**. Studio (the web interface) was added after the first candidate build.

## Verdict

Functionally ready for owner review as a release candidate. All six mandatory demonstrations and
all automated tests pass locally on Windows with a real PostgreSQL server; the 1,000,000-record
benchmark runs and verifies. **Open before a public release:** license decision, CI/Docker runs on
GitHub (not executable here), Linux/macOS verification, dependency audit execution.

## Environment used

* Windows 11, Intel Core i7-11800H (16 logical cores), 16 GB RAM, local SSD
* Rust 1.100.0 (pinned in `rust-toolchain.toml`), MSVC toolchain
* PostgreSQL 16.15 (isolated throwaway cluster on 127.0.0.1:55432, trust auth, created with the
  machine's existing PostgreSQL binaries; the existing system PostgreSQL service was not touched)
* Docker: not installed → `Dockerfile`/`docker-compose.yml` were **not built**

## Commands executed and results

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | ✅ clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ clean |
| `MA_TEST_PG_URL=postgres://ma_test@127.0.0.1:55432/postgres cargo test --workspace` | ✅ **82 passed, 0 failed** (74 in the first candidate; 4 regression tests added with the code-review fixes; 1 mapping test and 3 Studio end-to-end tests added with Studio) |
| `cargo run -p ma-bench --release` | ✅ all scenarios verified (see below) |
| `migrate-anything doctor` | ✅ all checks ok (state DB, keys, sandbox self-test) |
| README quickstart via the CLI binary (fresh workspace) | ✅ discovery, suggestion, plan, dry run; approval and job start correctly **refused** in a non-interactive shell (the interactive approval step must be performed by a human) |
| Studio in a browser (Chromium): demo from start to verified report, adding file / PostgreSQL / web API connections, connector generation, mapping edits, plan and test run, phone-width layout, animation toggle | ✅ checked by hand on a test build with synthetic data. Pause/resume and the approval rules are covered by `studio_ui.rs`; the Pause, Resume and Cancel buttons themselves were not clicked in a browser |
| `cargo audit`, `cargo deny`, gitleaks | ⚠️ configured in CI, **not executed** locally (tools not installed); license inventory checked manually via `cargo metadata` — all permissive |
| GitHub Actions (Linux/macOS/Windows matrix, Postgres service, release builds) | ⚠️ started by the push to the private repository; **result not checked** |

### Test inventory (82)

| Suite | Tests |
|---|---|
| ma-types (values, lossless JSON, hashing, property tests) | 10 |
| ma-security (encryption binding, SSRF policy, redaction) | 3 |
| ma-persistence (restart durability, leases) | 3 |
| ma-connector-sdk, ma-planning, ma-validation | 3 |
| ma-mapping (runtime, suggestions incl. Customer→Account example and key guessing, property test) | 8 |
| ma-sandbox (imports rejected, fuel, deadline, memory, OOB output) | 6 |
| ma-connector-factory (spec validation, malicious specs) | 2 |
| ma-connector-files (streaming, nested JSON, resume truncation, overwrite refusal, traversal) | 5 |
| ma-connector-postgres (1 unit + 9 integration incl. 2 regression tests) | 10 |
| ma-connector-rest (a create that failed in flight is not sent again) | 1 |
| ma-example-api (fixture API) | 8 |
| Demo 1 CRM | 1 |
| Demo 2 Finance | 1 |
| Demo 3 PostgreSQL → PostgreSQL | 1 |
| Demo 4 Unknown system | 1 |
| Demo 5 Interrupted (fault-injected crash, external kill, pause/resume/cancel, Postgres crash) | 4 |
| Demo 6 Security (12 scenarios incl. a real MCP stdio session) | 12 |
| Studio end to end over HTTP (sign-in to verified report, pause/resume, no approval without a terminal) | 3 |

## Demonstrations — measured outcomes

| Demo | Outcome |
|---|---|
| 1 CRM (CSV → PostgreSQL, different schema) | 3,800 written, 3,800 verified; 5,840/5,840 relationships preserved; self-reference written in second pass; **Completed and verified** |
| 2 Finance (JSON → JSONL) | 468 discovered, 458 written & verified, 10 failed (3 invalid-currency invoices + their 7 lines) and preserved in full; audit history archived, not re-created; **Completed with exceptions** (correct) |
| 3 PostgreSQL → PostgreSQL | 5,800 written & verified; 5,500/5,500 relationships; numeric sums identical; re-run produced no duplicates; **Completed and verified** |
| 4 Unknown API | connector generated from OpenAPI, 10/10 contract checks passed, injection text detected and withheld; API → files and API → second API both **Completed and verified**; HTTP 429s encountered and handled |
| 5 Interrupted | worker killed between destination write and checkpoint (files and PostgreSQL) and hard-killed externally; resume produced exactly 20,000 unique records each time; **Completed and verified** |
| 6 Security | unapproved, forged, expired and tampered approvals rejected; non-TTY approval refused; MCP `migrate_plan_approve` only marks *pending*; malicious specs rejected; metadata IP, loopback and non-allowlisted hosts denied; credential store unreachable from extensions; prompt injection without effect; tampered extension → job refuses to run |

## Benchmarks (release build)

See [`benchmarks/RESULTS.md`](../benchmarks/RESULTS.md) for the full table, hardware and workload.

| Scenario | Execute | Verify | Peak memory | Result |
|---|---|---|---|---|
| 1,000,000 records CSV → JSONL, 7 transformed fields | 14.7–25.7 s (≈39k–68k rec/s) | 24.4–25.3 s | ≈104–106 MB | Completed and verified |
| 100,000 records | 1.5–1.7 s | 2.3 s (one outlier run: 14.8 s) | ≈91 MB | Completed and verified |
| 20k parents + 200k children (references) | 4.5–4.6 s | 8.2–8.3 s | ≈104 MB | Completed and verified |
| 3 concurrent jobs × 100k | 4.5 s (≈67k rec/s aggregate) | 7.3 s | ≈208 MB | all verified |
| pause after 5 batches + resume, 200k | 3.3 s | — | — | no duplicates or gaps |
| transformation runtime alone (1 thread) | ≈420k rec/s | — | — | — |

Hardware: Intel i7-11800H, 16 logical cores, 16 GB RAM, Windows 11, local SSD. Throughput is
I/O-bound (per-batch fsync + `synchronous=FULL` SQLite commits; CPU 5–8 %) and varies between
runs; ranges from two consecutive full runs are shown.

## Defects found and fixed during this release candidate

Found by the verification engine, demos and benchmarks — each now has a regression test:

1. PostgreSQL keyset pagination ordered by the text-cast key → skipped/repeated rows (>9 pages).
2. PostgreSQL `char(n)` truncated to 1 character by the bind cast (`SEK` → `S  `).
3. JSON decimals routed through f64 (`10.10` → `10.1`) → replaced with a lossless parser.
4. Pause requested before a worker started was discarded.
5. Extension integrity check did not hash the loaded module bytes.
6. Prompt-injection text from API docs copied into generated specs → now withheld.
7. Verifier compared untyped destination text with typed expectations (false discrepancies).
8. Verification used non-indexable key lookups (≈14× slower) → keys always stored, indexed.
9. Empty file destinations could not be discovered/planned → supported with a 1:1 copy proposal.

## Code review (after the first commit)

A full review found 10 issues, all fixed: two security problems (an approved job could follow a
re-pointed connection; an agent could attach a stored credential to a connection it defined) and
eight correctness/robustness issues (see CHANGELOG "Fixed after code review"). Regression tests
cover the two security fixes, CSV resume before the first checkpoint, the plus-sign inference,
the REST re-send, text comparison in verification, the redaction panic and the stale error. The
lease heartbeat and the download cap have **no dedicated test**.

## Security risks (residual)

* Same-OS-user agent with unrestricted shell + file access could read the approval key (see
  SECURITY.md). Mitigation: run agents under a separate account/container.
* Contract tests do not exercise writes; first writes to a generated connector happen in an
  approved migration (documented; capability shown as *Documented*).
* Dependency vulnerability audit not executed locally.

## Unfinished / not in v1 (accurately reported, not claimed)

GraphQL/other databases/Excel/XML connectors; OAuth 2.0; OpenAPI 2.0; attachment transfer;
external LLM mapping providers (trait exists, none shipped); remote MCP; PostgreSQL state backend;
process-isolated extensions (fails closed instead); Polars comparison benchmark; CI/Docker
execution; Linux/macOS verification; final license.

## Next steps for the owner

1. Run `migrate-anything plan approve <plan>` once yourself to experience the approval flow
   (it requires your interactive terminal by design).
2. Decide the license (docs/LICENSE_DECISION_CHECKLIST.md).
3. Look at the CI run on GitHub (multi-OS tests, audit, deny, gitleaks, release artifacts); it
   was started by the push and has not been checked.
4. Start `studio.cmd demo`, approve with the code from the terminal and watch a run: this is
   the one path only a person can exercise end to end in a browser.
