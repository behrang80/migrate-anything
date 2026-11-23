# Contributing

Thanks for helping. Migrate Anything is licensed under [Apache-2.0](LICENSE). Anything you
contribute is accepted under that same license (section 5 of the license), so only send work you
have the right to contribute. No separate contributor agreement is asked for today; the owner may
add a sign-off requirement later (see docs/LICENSE_DECISION_CHECKLIST.md).

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

## Working on Studio (the web interface)

Studio is `apps/ui-server` (crate `ma-ui`): a small local HTTP server over `ma_core::App` plus
plain HTML, CSS and JavaScript modules in `apps/ui-server/static`. There is no build step and no
JavaScript dependency; the files are embedded in the binary with `include_str!`.

```sh
# serve the interface files from disk, so an edit shows up on reload (no rebuild)
MA_UI_DEV_DIR=apps/ui-server/static cargo run -p migrate-anything -- ui
cargo test -p migrate-anything --test studio_ui     # sign-in, demo, approval code, run, proof
```

* Colours, spacing and type sizes are tokens at the top of `app.css`; use them instead of raw
  values. Icons are inline SVG (no emoji). Every control has a visible label and a focus ring.
* The code rain is the one decorative animation. It must keep honouring the Animation toggle
  and `prefers-reduced-motion`, and every number it stands for must also be on screen as text.
* The page may not load anything from another origin (the Content-Security-Policy forbids it).
* Anything that approves goes through a terminal code (`InteractiveSession::challenge`). Never
  add a way to approve from the browser alone.
* Behaviour changes need a case in `apps/cli/tests/studio_ui.rs`, which drives the same HTTP API
  the page uses.

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
