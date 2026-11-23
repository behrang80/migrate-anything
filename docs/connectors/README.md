# Connectors

| Kind | Read | Write | Discovery | Resume / idempotency | Notes |
|---|---|---|---|---|---|
| `files` | CSV, JSON array, JSON Lines (streaming, byte-offset cursors) | CSV, JSON, JSONL | types inferred from up to 1,000 rows; `<x>_id` relationship hints (Inferred) | output truncated to the checkpointed length on resume | ASCII-compatible encodings; CSV cannot distinguish null/empty; refuses to overwrite non-empty files unless `overwrite: true` (destructive) |
| `postgres` | keyset pagination on single-column PKs; OFFSET otherwise | upsert by unique key (`ON CONFLICT`), insert, update (second pass) | catalog: columns, types, NOT NULL/defaults, identity/generated, PK, unique keys, FKs (Documented), enums, comments, counts | upserts in one transaction per batch; bad rows isolated with savepoints | values read as text and parsed per column type (no float rounding of numerics) |
| `rest` | declarative spec: cursor / offset / page pagination | POST create; upsert = lookup by key then PATCH/PUT or POST | from the spec (generated from OpenAPI) | idempotent only with lookup; otherwise in-flight records flagged for reconciliation | DELETE never issued; per-request retries on 429/5xx honoring Retry-After |

## Settings

```jsonc
// files
{"path": "./data", "format": "csv", "delimiter": ",", "encoding": "utf-8",
 "id_fields": {"customers": "customer_no"}, "empty_as_null": true, "overwrite": false}
// postgres (password via --credential-ref)
{"url": "postgres://user@host:5432/db", "schema": "public"}
// rest (credential via --credential-ref, hosts via --allow-host)
{"base_url": "https://api.example.com", "allow_private_network": false}
```

## SDK

`crates/connector-sdk` defines small traits a connector implements as far as the system allows:

* `Connector` — `identify`, `capabilities` (evidence-graded manifest), `check_connection`,
  `discover_schema`, `connector_version`, `creates_objects_on_write`.
* `RecordSource` — `read_batch(object, cursor, limit) -> Batch { records, next_cursor }`, `count`.
* `RecordSink` — `prepare(spec, resume_token)`, `write_batch -> Vec<Result<dest_id>>`
  (`Err` for the whole batch only when *nothing* was written), `durable_token`, `update_fields`,
  `finish`, `idempotent_rewrites(mode, has_key)`, `supports_updates`.
* `RecordLookup` — `lookup_by_key(object, key_field, key)`.

The cursor returned with a batch is the resume point; the engine stores it atomically with the
batch outcomes. A sink's `durable_token` is stored with it and handed back on resume.

See [DECLARATIVE_SPEC.md](DECLARATIVE_SPEC.md) for generated REST connectors.
