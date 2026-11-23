# Examples

| Folder | Contents | Used by |
|---|---|---|
| `crm/data` | Small CSV export of a legacy CRM (organizations, contacts with a self-referencing manager, opportunities; unicode, quoted commas, empty values). `mapping.copy-to-files.json` is the reviewed 1:1 mapping produced by `mapping suggest` for a new file destination. | Quickstart; Demo 1 uses a larger generated version |
| `finance/data` | JSON customers and invoices with exact decimals, localized dates, an invalid currency and audit history. | Demo 2 uses a larger generated version |
| `postgres` | `source.sql` / `destination.sql`: schema-different databases with foreign keys and synthetic data. | Demo 3 |
| `example-api` | Rust crate: "FieldOps Legacy Service", a self-contained REST API with an OpenAPI document (cursor pagination, API key, 429 rate limiting, prompt-injection text in its docs). No platform dependencies. `cargo run -p ma-example-api -- --port 8088` | Demo 4, security tests |
| `generated-api` | Walkthrough for generating a connector for the example API. | — |
| `mcp` | MCP client configuration. | — |
