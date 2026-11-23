# License: decision and remaining checklist

**Decided by the owner on 2026-11-23: Apache License 2.0.**
Copyright holder: Behrang Karimi (see `NOTICE`).

What that means in practice:

* Anyone may use, change and redistribute the software, also commercially and inside closed
  products, as long as they keep the license text and the `NOTICE` file and mark changed files.
* The license includes a patent grant from every contributor and ends that grant for anyone who
  sues over patents in the software.
* There is no warranty and no liability.
* The license does not earn money by itself. Revenue has to come from something sold next to the
  open code: support, hosting, paid add-ons, or services. Apache-2.0 allows the owner (and anyone
  else) to build closed add-ons on top of it.

This file is not legal advice. Have a lawyer confirm the points below that matter to you before
the repository is made public.

## 1. Done
- [x] `LICENSE` holds the unmodified Apache-2.0 text.
- [x] `NOTICE` names the project and the copyright holder.
- [x] `license = "Apache-2.0"` in `[workspace.package]`; every crate inherits it.
- [x] README and CONTRIBUTING state the license.
- [x] `cargo deny check licenses bans sources` passes (run locally on 2026-11-23).

## 2. Dependency compatibility (checked 2026-11-23 with `cargo deny`)
All third-party crates are under permissive licenses that are compatible with Apache-2.0:
MIT, Apache-2.0 (incl. LLVM exception), BSD-2/3-Clause, ISC, Zlib, Unicode-3.0,
CDLA-Permissive-2.0, BSL-1.0 (Boost), Unlicense (dual with MIT).
One crate is `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; MIT/Apache is selected.
SQLite is bundled via `libsqlite3-sys` (SQLite itself is public domain).
`ring` is under Apache-2.0 AND ISC (plus historical OpenSSL-derived notices).

- [ ] Binary downloads must ship third-party notices (Apache-2.0 NOTICE requirements, MIT/BSD
      attribution). Generate them with `cargo about` and add a THIRD_PARTY_NOTICES file to every
      release archive. **Not done yet; needed before the first binary release.**
- [x] `cargo deny check licenses` runs in CI on every push (configured in `deny.toml`).

## 3. Contribution rights
- [x] Contributions are accepted under Apache-2.0 (section 5 of the license: inbound = outbound).
- [ ] Optional: require a DCO sign-off (`Signed-off-by`) so every contributor states they have the
      right to contribute. A CLA is only needed if the owner later wants to relicense the whole
      code base under different terms.

## 4. Trademarks and naming
Apache-2.0 does not give anyone the right to use the project's name or logo (section 6).
- [ ] Check availability of "Migrate Anything" as a product name/trademark in target markets.
- [ ] Decide a trademark policy for forks.

## 5. Ownership
- [ ] Confirm that the owner holds the rights personally (not an employer or a company). Much of
      the code was written with an AI assistant; how far such code is protected by copyright is
      not settled law. A lawyer can say whether that matters for your plans.

## 6. Distribution
- [ ] Repository visibility (private today) and release channels (GitHub Releases, crates.io:
      `publish = false` is set everywhere today), container registry.
- [ ] Export-control review for cryptography (ChaCha20-Poly1305, HMAC-SHA256, TLS) if required in
      your jurisdiction.
- [ ] Privacy statement: the software is local-first and sends no telemetry.
- [ ] Optional: SPDX headers (`// SPDX-License-Identifier: Apache-2.0`) in source files.
