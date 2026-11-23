# Generating a connector for an unknown API

```sh
cargo run -p ma-example-api -- --port 8088 &          # API key: test-key-123
migrate-anything credentials set fieldops-key            # enter test-key-123
migrate-anything connections add fieldops --kind rest --env test \
  --settings '{"base_url":"http://127.0.0.1:8088","allow_private_network":true}' \
  --credential-ref fieldops-key --allow-host 127.0.0.1
migrate-anything connectors generate --connection fieldops --id fieldops-legacy \
  --openapi-url http://127.0.0.1:8088/openapi.json
migrate-anything connectors test --connection fieldops
migrate-anything systems discover fieldops
```

Expected: resources `technicians` (identity `tech_id`) and `work_orders` (identity `wo_number`),
cursor pagination (`after` / `next_cursor`), API-key header auth, an inferred relationship
`work_orders.assigned_tech → technicians.tech_id`, and a warning that the document contains text
addressed to AI agents (it is withheld from the spec and has no effect).
The automated version of this walkthrough is `apps/cli/tests/demo_unknown_api.rs`.
