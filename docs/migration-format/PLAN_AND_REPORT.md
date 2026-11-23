# Plans, approvals and reports

## Plan (`MigrationPlan`, format version 1)
Immutable JSON containing: source/destination endpoints (connection, identity, connector id and
version, environment), selected source and destination schemas, the mapping, execution order,
deferred (second-pass) references, estimated record counts, operations (`read`, `insert`,
`upsert`, `update`, `truncate_destination`), unresolved mappings, risks (`info`/`warning`/
`blocking`), verification strategy, recovery strategy, approval requirements, execution options
and pinned extension hashes. The plan hash is SHA-256 over canonical JSON (keys sorted, no
whitespace). A Markdown rendering is stored with the plan and shown on approval.

## Approval (`ApprovalRecord`)
Claims: approval id, workspace, plan id + hash, source and destination identities, connector refs,
permitted operations, `destructive_authorized`, operator, verification method
(`interactive-tty-challenge`), target environment, issued/expires. Signature: HMAC-SHA256 over the
canonical claims with the workspace approval key. Verified at job start and again by the worker.

## Report (`MigrationReport`)
```
MIGRATION REPORT
Source system / Destination system / Migration plan (+hash) / Job
Execution start / Execution end
Records discovered / eligible / attempted / written / verified / skipped / failed / requiring review
Relationships preserved (of checked) / Attachments migrated / Archived source values
Unsupported fields / Unresolved discrepancies
Final status
```
Final status is computed from stored evidence only:
* **Completed and verified** — job completed, verification ran, no failures, no archived or
  skipped values, no discrepancies.
* **Completed with exceptions** — job completed and verified, but records failed, values were
  archived/skipped, or checks found discrepancies.
* **Partially completed** — job failed/stopped after writing, or verification found written
  records missing from the destination.
* **Failed** / **Cancelled** / **Not verified** (completed but `verify` not run yet).

Records that failed transformation are not counted as *attempted* (no write was tried).
