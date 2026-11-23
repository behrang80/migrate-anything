# Contributing

Thanks for helping. Before external contributions are accepted the owner must finalize the
license and contribution terms (see docs/LICENSE_DECISION_CHECKLIST.md — a CLA or DCO decision is
part of it). Until then, issues and discussion are welcome; pull requests may not be merged.

## Development

```sh
rustup show                     # uses rust-toolchain.toml (1.100.0)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
MA_TEST_PG_URL=postgres://user@127.0.0.1:5432/postgres cargo test --workspace   # PostgreSQL demos
cargo run -p ma-bench --release -- --quick
```

On Windows a throwaway PostgreSQL can be run without installing a service:
`initdb -D .local/pgdata -U ma_test --auth=trust` then
`pg_ctl -D .local/pgdata -o "-p 55432 -c listen_addresses=127.0.0.1" start`.

## Rules of the codebase

* Business logic lives in `crates/`; `apps/` are protocol adapters only.
* No system-pair-specific logic in the universal core. New systems are connectors.
* Never claim success without evidence: every behaviour change needs a test that would fail
  without it. Prefer end-to-end tests in `apps/cli/tests/` for user-visible behaviour.
* No silent data loss: any value that cannot be written must be archived or reported.
* Security-relevant changes (approvals, network policy, sandbox, secrets) need a test in
  `apps/cli/tests/demo_security.rs` or the owning crate.
* New dependencies must be justified, license-compatible (`cargo deny check`) and pinned through
  `Cargo.lock`.
* Never commit secrets, real customer data or confidential documents.

## Writing a connector

Implement the traits in `crates/connector-sdk` that your system supports (`Connector` plus any of
`RecordSource`, `RecordSink`, `RecordLookup`), report capabilities with honest evidence levels,
and wire construction into `ma_core::App::connect`. For REST systems prefer a declarative spec
(see docs/connectors/DECLARATIVE_SPEC.md) over native code.
