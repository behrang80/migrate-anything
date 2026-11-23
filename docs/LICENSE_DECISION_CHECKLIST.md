# License decision checklist (for the owner)

No license has been selected. `LICENSE` currently reserves all rights. Nothing has been published.
Decide the points below before any public release.

## 1. Target model
- [ ] Proprietary / commercial only
- [ ] Source-available (e.g. Business Source License 1.1, Elastic License 2.0, PolyForm family,
      Functional Source License) — **not OSI open source**; do not describe it as such
- [ ] Open source (OSI-approved: Apache-2.0, MIT, MPL-2.0, AGPL-3.0, …)
- [ ] Dual licensing (e.g. AGPL-3.0 + commercial)

Questions to answer: Is hosted/SaaS use by third parties allowed? Is competing use restricted?
Should the license convert to open source after a period (BSL "change date")? Are generated
connectors and specs produced by users owned by users (recommended: yes, state it explicitly)?

## 2. Dependency compatibility (checked 2026-11-23 from `cargo metadata`)
All third-party crates are under permissive licenses:
MIT, Apache-2.0 (incl. LLVM exception), BSD-2/3-Clause, ISC, Zlib, Unicode-3.0,
CDLA-Permissive-2.0, BSL-1.0 (Boost), Unlicense (dual with MIT).
One crate is `MIT OR Apache-2.0 OR LGPL-2.1-or-later` — select MIT/Apache.
SQLite is bundled via `libsqlite3-sys` (SQLite itself is public domain).
`ring` is under Apache-2.0 AND ISC (plus historical OpenSSL-derived notices).

Implications:
- [ ] Any of the models above is compatible with these dependencies.
- [ ] Binary distributions must ship third-party notices (Apache-2.0 NOTICE requirements, MIT/BSD
      attribution). Generate with `cargo about` or `cargo deny list` and add a THIRD_PARTY_NOTICES file.
- [ ] Re-run `cargo deny check licenses` in CI before every release (configured in `deny.toml`).

## 3. Contribution rights
- [ ] Choose CLA (needed for dual/commercial relicensing) or DCO (`Signed-off-by`).
- [ ] Update CONTRIBUTING.md accordingly; until then external PRs are not merged.

## 4. Trademarks and naming
- [ ] Check availability of "Migrate Anything" as a product name/trademark in target markets.
- [ ] Decide trademark policy for forks.

## 5. Distribution
- [ ] Repository visibility, release channels (GitHub Releases, crates.io — `publish = false` is
      set everywhere today), container registry.
- [ ] Export-control review for cryptography (ChaCha20-Poly1305, HMAC-SHA256, TLS) if required in
      your jurisdiction.
- [ ] Privacy statement: the software is local-first and sends no telemetry.

## 6. After the decision
- [ ] Replace `LICENSE`, set `license = "<SPDX>"` in `[workspace.package]` (replacing `license-file`).
- [ ] Add SPDX headers if desired; update README "License" section.
