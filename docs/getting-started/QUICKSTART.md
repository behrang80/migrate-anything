# Quickstart

## The easy way: Studio

Studio is the web interface. It runs on your own computer and opens in your browser.

```sh
studio.cmd demo        # Windows (or double-click studio.cmd)
./studio.sh demo       # macOS / Linux
```

The first start builds the program. Then Studio opens with a demo migration prepared (90,000
made-up CRM records that never leave your computer):

1. **Map** — look at how the fields were matched, then press *Create the plan*.
2. **Plan** — read what will happen; a test run has already checked a sample without writing.
3. **Approve** — press *Show a code in my terminal*, read the 6-character code in the terminal
   window Studio was started from, type it in the browser.
4. **Run & proof** — watch the migration, then read the verified report. *Watch it again*
   replays a finished run.

For your own data start Studio without `demo` and press *Start a migration*. Add a connection
on each side:

* **Files** — a folder with one file per data set (`customers.csv`, `orders.csv`). Studio
  proposes the ID column of each file; confirm it so records can be matched and verified.
* **PostgreSQL** — server, database, user, password. As a destination the tables must exist.
* **Web API** — the address and, if needed, a key. Studio then asks for the API's OpenAPI
  description (usually `/openapi.json`) and builds and tests a connector from it.

Keep the terminal window open while you work: it shows the approval codes, and closing it stops
Studio (a running migration continues in its own process and is there when you start Studio
again).

The rest of this page does the same with commands.

## 1. Install and verify

```sh
cargo install --path apps/cli --locked     # Rust 1.100+; Windows needs MSVC build tools
migrate-anything doctor
```

State lives in `~/.migrate-anything` (override with `--home` or `MA_HOME`): `state.db` (SQLite),
`keys/` (secret and approval keys), `logs/` (worker logs). Protect it like your data.

## 2. Files → files (sample data in `examples/crm/data`)

```sh
migrate-anything connections add legacy --kind files --env production \
  --settings '{"path":"examples/crm/data","format":"csv","id_fields":{"organizations":"org_id","contacts":"contact_id","opportunities":"opp_id"}}'
migrate-anything connections add target --kind files --env test --settings '{"path":"out","format":"jsonl"}'
migrate-anything systems discover legacy
migrate-anything mapping suggest --source legacy --dest target --out mapping.json
#   review mapping.json (see docs/migration-format/MAPPING.md), then:
migrate-anything plan create --source legacy --dest target --mapping mapping.json
migrate-anything plan preview <plan-id> --dry-run       # read-only
migrate-anything plan approve <plan-id>                 # interactive: type the shown code
migrate-anything jobs start <plan-id>                   # detached worker; add --foreground to watch
migrate-anything jobs status <job-id>
migrate-anything verify <job-id>
migrate-anything report <job-id>
migrate-anything exceptions <job-id>                    # archived values and records not written
```

## 3. PostgreSQL

```sh
migrate-anything credentials set pg-legacy               # prompts; stored encrypted
migrate-anything connections add old-db --kind postgres --env production \
  --settings '{"url":"postgres://migrator@db.internal:5432/erp","schema":"public"}' --credential-ref pg-legacy
```

Never put passwords in URLs — they are rejected.

## 4. An unknown REST system

```sh
migrate-anything credentials set api-key
migrate-anything connections add vendor --kind rest --env production \
  --settings '{"base_url":"https://api.vendor.example"}' --credential-ref api-key --allow-host api.vendor.example
migrate-anything connectors generate --connection vendor --id vendor-api --openapi-url https://api.vendor.example/openapi.json
migrate-anything connectors test --connection vendor        # read-only contract tests → "tested"
migrate-anything connectors activate vendor-api@1.0.0       # operator; required to WRITE to it
```

Try it locally with the bundled fixture: `cargo run -p ma-example-api -- --port 8088` (API key
`test-key-123`), base URL `http://127.0.0.1:8088`, `--allow-host 127.0.0.1` and
`"allow_private_network": true` in the settings (loopback is denied by default).

## 5. From an AI client

```sh
claude mcp add migrate-anything -- migrate-anything mcp serve --allow-dir ./exports
```

`--allow-dir <folder>` (repeatable, or the `MA_AGENT_DIRS` environment variable) opens a folder to
the agent: it can register file connections inside it and read OpenAPI documents from it. Without
it the agent works only with connections you added. Database connections, credentials, private
network addresses and replacing a connection's connector always need you.

Ask: *"Discover the `legacy` and `target` connections, propose a mapping, create a plan, show me
the dry run, and request approval."* Then approve in your terminal and let the agent run and
verify the job. The agent cannot approve, activate connectors, or read credentials.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `approval requires an interactive terminal` | Run `plan approve` yourself in a real terminal (not via a script or agent). |
| `has no operator approval; writes are refused` | Approve the plan; any plan change requires a new plan + approval. |
| `host ... is not on this connection's allowlist` | Add `--allow-host` for that exact host. |
| `address ... is private/loopback` | Set `"allow_private_network": true` in the connection settings (only for trusted local systems). |
| `destination file ... already exists` | Remove it, or set `"overwrite": true` and approve with `--allow-destructive`. |
| `mappings need review` | Edit the mapping: set `"status": "accepted"` (or `"rejected"`) for `needs_review` fields. |
| Job stuck `running` after a crash | `migrate-anything jobs resume <job-id>` (waits for the dead worker's lease to expire, 30 s). |
| Report says *Completed with exceptions* | See `migrate-anything exceptions <job-id>`; nothing was dropped silently. |
