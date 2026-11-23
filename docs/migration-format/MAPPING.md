# Mapping specification

A `MappingSpec` is the deterministic, reviewable contract for how source records become
destination records. AI or heuristic suggestions are only ever *converted into* this format;
nothing executes a suggestion directly.

```jsonc
{
  "mapping_version": 1,
  "objects": [{
    "source_object": "invoices",
    "target_object": "bills",
    "target_key_field": "legacy_invoice_id",     // holds the source id: idempotent upserts + verification
    "write_mode": "upsert",                       // or "insert"
    "unmapped_fields": "archive",                 // or "skip" (reported as skipped)
    "skip_fields": [],
    "fields": [
      { "target": "legacy_invoice_id", "confidence": 1.0, "status": "accepted",
        "expr": { "op": "cast", "input": { "op": "field", "path": "invoice_id" }, "to": "string" } },
      { "target": "client_ref", "confidence": 0.93, "status": "accepted",
        "explanation": "…", "evidence": ["…"],
        "expr": { "op": "ref", "input": { "op": "field", "path": "customer_no" }, "object": "customers" } },
      { "target": "total_minor", "confidence": 1.0, "status": "accepted",
        "expr": { "op": "scale_decimal", "input": { "op": "field", "path": "total" }, "pow": 2 } }
    ]
  }]
}
```

`status`: `accepted`, `needs_review` (blocks approval), `rejected` (ignored).

## Expressions (`op`)

| op | fields | semantics |
|---|---|---|
| `field` | `path` (dotted, array indexes allowed) | missing stays missing; null stays null |
| `const` | `value` (typed value, e.g. `{"type":"string","value":"x"}`) | literal |
| `cast` | `input`, `to` (`string,int,float,decimal,bool,date,date_time,bytes,array,object`) | failure → invalid → record fails (never silently dropped) |
| `concat` | `parts`, `separator` | skips missing/null parts |
| `split` | `input`, `separator`, `index` | |
| `enum_map` | `input`, `map`, `on_unmapped` (`fail`, `null_and_archive`, `keep`) | |
| `coalesce` | `inputs` | first present non-null |
| `default` | `input`, `value` | when missing or null |
| `trim` / `lower` / `upper` | `input` | |
| `parse_date` | `input`, `format` (chrono), `datetime` | |
| `scale_decimal` | `input`, `pow` | exact ×10^pow (currency minor units) |
| `unit_convert` | `input`, `factor` (decimal text) | exact multiplication |
| `ref` | `input`, `object` | source id → destination id via the job's id map; unresolved → record fails |
| `extension` | `input`, `name` | registered WebAssembly extension in the sandbox |
| `object` | `fields` (map of expressions) | builds a nested object |

## Outcomes per record
`migrated_natively` (only copies/lossless casts), `transformed`, `written_with_archive`,
`skipped_intentionally`, `unsupported`, `failed`, `requires_reconciliation`. Records that are not
written are stored in full in the exception archive (`migrate-anything exceptions <job>`).
