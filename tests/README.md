# Tests

* Unit tests live next to the code in each crate (`cargo test -p <crate>`).
* Integration tests for connectors: `connectors/postgres/tests`, `examples/example-api/tests`.
* End-to-end demonstrations and security tests: `apps/cli/tests/` (placed in the CLI package so
  they can run the real `migrate-anything` binary as worker and MCP server):
  `demo_crm.rs`, `demo_finance.rs`, `demo_postgres.rs`, `demo_unknown_api.rs`,
  `demo_interrupted.rs`, `demo_security.rs`.
* PostgreSQL tests need `MA_TEST_PG_URL` (a superuser URL; each test creates and drops its own
  database). Without it they print `skipped: MA_TEST_PG_URL not set` and pass vacuously — CI runs
  them with a PostgreSQL service.
* Fixture data is generated deterministically inside the tests; `tests/fixtures/` is reserved for
  static fixtures.
