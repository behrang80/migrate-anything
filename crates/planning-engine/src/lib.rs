//! Plan construction. Pure and deterministic given its inputs (no I/O): the application layer
//! gathers schemas, estimates and endpoint identities, and this crate validates the mapping,
//! orders objects by dependency, breaks cycles with deferred second-pass references, assesses
//! risks and produces an immutable, hashable [`MigrationPlan`].

use std::collections::{BTreeMap, BTreeSet};

use ma_types::*;

pub struct PlanInput {
    pub workspace: String,
    pub source: PlanEndpoint,
    pub destination: PlanEndpoint,
    pub source_schema: SystemSchema,
    pub destination_schema: SystemSchema,
    pub mapping: MappingSpec,
    pub options: ExecutionOptions,
    pub estimates: BTreeMap<String, Option<u64>>,
    pub dest_creates_objects: bool,
    pub dest_supports_updates: bool,
    /// `idempotent_rewrites` per target object as reported by the destination sink.
    pub dest_idempotent: BTreeMap<String, bool>,
    pub dest_limitations: Vec<String>,
    pub destructive_operations: Vec<Operation>,
    pub extensions: BTreeMap<String, String>,
}

fn risk(severity: Severity, code: &str, message: impl Into<String>, object: Option<&str>) -> Risk {
    Risk { severity, code: code.into(), message: message.into(), object: object.map(String::from) }
}

/// Build the plan. Errors only for structurally impossible inputs; everything else is a risk.
pub fn build_plan(input: PlanInput) -> MaResult<(MigrationPlan, String)> {
    let mut mapping = input.mapping.clone();
    let mut risks = vec![];
    let mut unresolved = vec![];

    if mapping.objects.is_empty() {
        return Err(MaError::validation("the mapping contains no objects"));
    }
    let mut seen = BTreeSet::new();
    for om in &mapping.objects {
        if !seen.insert(om.source_object.clone()) {
            return Err(MaError::validation(format!("source object `{}` is mapped twice", om.source_object)));
        }
    }

    // ---------------------------------------------------------------- per-object validation
    for om in &mapping.objects {
        let so = input.source_schema.object(&om.source_object);
        let to = input.destination_schema.object(&om.target_object);
        let o = Some(om.source_object.as_str());
        let Some(so) = so else {
            risks.push(risk(
                Severity::Blocking,
                "unknown_source_object",
                format!("source object `{}` does not exist", om.source_object),
                o,
            ));
            continue;
        };
        if to.is_none() && !input.dest_creates_objects {
            risks.push(risk(
                Severity::Blocking,
                "unknown_destination_object",
                format!("destination object `{}` does not exist and the destination cannot create objects", om.target_object),
                o,
            ));
        }
        let mut targets = BTreeSet::new();
        for fm in &om.fields {
            if !targets.insert(fm.target.clone()) {
                risks.push(risk(Severity::Blocking, "duplicate_target", format!("destination field `{}` is mapped twice", fm.target), o));
            }
            if fm.status == ReviewStatus::NeedsReview {
                unresolved.push(format!(
                    "{}.{} ← {} (confidence {:.2}): {}",
                    om.target_object,
                    fm.target,
                    expr_summary(&fm.expr),
                    fm.confidence,
                    fm.explanation
                ));
            }
            if fm.status == ReviewStatus::Rejected {
                continue;
            }
            let mut paths = vec![];
            fm.expr.source_paths(&mut paths);
            for p in paths {
                let top = p.split('.').next().unwrap_or_default();
                if so.field(top).is_none() {
                    risks.push(risk(
                        Severity::Blocking,
                        "unknown_source_field",
                        format!("`{}` reads `{p}`, which is not a field of source object `{}`", fm.target, so.name),
                        o,
                    ));
                }
            }
            if let Some(to) = to {
                match to.field(&fm.target) {
                    None if !input.dest_creates_objects => risks.push(risk(
                        Severity::Blocking,
                        "unknown_destination_field",
                        format!("destination object `{}` has no field `{}`", to.name, fm.target),
                        o,
                    )),
                    Some(tf) if tf.read_only => risks.push(risk(
                        Severity::Blocking,
                        "read_only_destination_field",
                        format!("destination field `{}.{}` is read-only (system generated)", to.name, fm.target),
                        o,
                    )),
                    Some(tf) => {
                        if let Expr::Field { path } = &fm.expr
                            && let Some(sf) = so.field(path)
                            && !sf.field_type.converts_to(tf.field_type)
                        {
                            risks.push(risk(
                                Severity::Warning,
                                "type_mismatch",
                                format!(
                                    "`{}` ({:?}) copied into `{}` ({:?}) without a conversion",
                                    sf.name, sf.field_type, tf.name, tf.field_type
                                ),
                                o,
                            ));
                        }
                    }
                    None => {}
                }
            }
            let mut refs = vec![];
            fm.expr.referenced_objects(&mut refs);
            for r in refs {
                if mapping.object(&r).is_none() {
                    risks.push(risk(
                        Severity::Blocking,
                        "reference_not_migrated",
                        format!("`{}` references `{r}`, which is not part of this plan; references could not be translated", fm.target),
                        o,
                    ));
                }
            }
            let mut exts = vec![];
            fm.expr.extensions(&mut exts);
            for e in exts {
                if !input.extensions.contains_key(&e) {
                    risks.push(risk(Severity::Blocking, "unknown_extension", format!("extension `{e}` is not registered"), o));
                }
            }
        }
        if let Some(to) = to {
            for f in to.fields.iter().filter(|f| f.required && !f.read_only) {
                if !om.fields.iter().any(|m| m.target == f.name && m.status != ReviewStatus::Rejected) {
                    risks.push(risk(
                        Severity::Blocking,
                        "required_field_unmapped",
                        format!("required destination field `{}.{}` has no mapping", to.name, f.name),
                        o,
                    ));
                }
            }
        }
        match (om.write_mode, &om.target_key_field) {
            (WriteMode::Upsert, None) => risks.push(risk(
                Severity::Blocking,
                "upsert_without_key",
                "upsert requires `target_key_field` (a destination field holding the source id)",
                o,
            )),
            (_, Some(k)) if !om.fields.iter().any(|f| &f.target == k) => {
                risks.push(risk(Severity::Blocking, "key_field_unmapped", format!("key field `{k}` has no mapping"), o))
            }
            (WriteMode::Insert, None) => risks.push(risk(
                Severity::Warning,
                "no_external_key",
                "no external key: verification can only compare counts, and a crash during a write may duplicate records",
                o,
            )),
            _ => {}
        }
        if !input.dest_idempotent.get(&om.target_object).copied().unwrap_or(false) {
            risks.push(risk(
                Severity::Warning,
                "non_idempotent_destination",
                "destination writes are not idempotent for this object: records of a batch interrupted mid-write are marked `requires_reconciliation` instead of being re-sent",
                o,
            ));
        }
        let unmapped: Vec<String> = so
            .fields
            .iter()
            .filter(|f| !om.source_paths().iter().any(|p| p.split('.').next() == Some(f.name.as_str())))
            .map(|f| f.name.clone())
            .collect();
        let (skipped, archived): (Vec<_>, Vec<_>) = unmapped.into_iter().partition(|f| om.skip_fields.contains(f));
        if !archived.is_empty() {
            let how = match om.unmapped_fields {
                UnmappedFieldPolicy::Archive => "will be preserved in the exception archive",
                UnmappedFieldPolicy::Skip => "will be SKIPPED (not migrated, not archived)",
            };
            risks.push(risk(Severity::Info, "unmapped_source_fields", format!("unmapped source fields {how}: {}", archived.join(", ")), o));
        }
        if !skipped.is_empty() {
            risks.push(risk(Severity::Info, "skipped_fields", format!("intentionally skipped fields: {}", skipped.join(", ")), o));
        }
    }
    if !unresolved.is_empty() {
        risks.push(risk(
            Severity::Blocking,
            "mappings_need_review",
            format!("{} field mapping(s) need review before approval", unresolved.len()),
            None,
        ));
    }
    for l in &input.dest_limitations {
        risks.push(risk(Severity::Warning, "destination_limitation", l.clone(), None));
    }

    // ---------------------------------------------------------------- dependency order
    let (order, deferred) = order_objects(&mut mapping);
    if !deferred.is_empty() && !input.dest_supports_updates {
        risks.push(risk(
            Severity::Blocking,
            "cycle_requires_updates",
            format!(
                "cyclic references ({}) require a second update pass, which the destination does not support",
                deferred.iter().map(|d| format!("{}.{}", d.source_object, d.target_field)).collect::<Vec<_>>().join(", ")
            ),
            None,
        ));
    }

    // ---------------------------------------------------------------- operations
    let mut operations = vec![Operation::Read];
    for om in &mapping.objects {
        operations.push(match om.write_mode {
            WriteMode::Upsert => Operation::Upsert,
            WriteMode::Insert => Operation::Insert,
        });
    }
    if !deferred.is_empty() {
        operations.push(Operation::Update);
    }
    operations.extend(input.destructive_operations.iter().copied());
    operations.sort();
    operations.dedup();

    let mut approval_requirements = vec![
        "operator approval via `migrate-anything plan approve` on an interactive terminal".to_string(),
        format!("approval bound to plan hash, systems, connector versions and target environment `{}`", input.destination.environment),
    ];
    if operations.iter().any(|o| o.is_destructive()) {
        approval_requirements.push("separate authorization for destructive operations (`--allow-destructive`)".into());
    }
    if input.source.connector.generated || input.destination.connector.generated {
        approval_requirements.push("generated connectors must be tested and activated by an operator".into());
    }

    let verification_strategy = vec![
        "compare source counts, eligible records, written records and destination records per object".into(),
        "identifier coverage: every id-map entry must be found in the destination by its external key".into(),
        "duplicate detection on destination external keys".into(),
        "referential integrity: every migrated reference must point at a migrated destination record".into(),
        format!(
            "re-transform up to {} sampled source records and compare field-by-field with the destination",
            input.options.verify_sample_size
        ),
        "order-independent content checksum over all mapped fields of written records".into(),
    ];
    let recovery_strategy = vec![
        "durable checkpoint after every batch (cursor, sink position, outcomes and id map in one transaction)".into(),
        "worker lease with heartbeat; a restarted worker resumes from the last checkpoint".into(),
        match input.destination.identity.kind.as_str() {
            "files" => "file outputs are truncated back to the checkpointed length before resuming".into(),
            "postgres" => "writes are upserts by external key inside a transaction; re-sending a batch cannot duplicate".into(),
            _ => "for non-idempotent APIs, records of an interrupted batch are looked up by key (if supported) or flagged for manual reconciliation".into(),
        },
    ];

    let plan = MigrationPlan {
        plan_format_version: PLAN_FORMAT_VERSION,
        plan_id: format!("plan-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        workspace: input.workspace,
        created_at: chrono::Utc::now(),
        source: input.source,
        destination: input.destination,
        source_schema: select_objects(&input.source_schema, mapping.objects.iter().map(|o| o.source_object.as_str())),
        destination_schema: select_objects(&input.destination_schema, mapping.objects.iter().map(|o| o.target_object.as_str())),
        estimated_records: order.iter().map(|o| (o.clone(), input.estimates.get(o).copied().flatten())).collect(),
        mapping,
        execution_order: order,
        deferred_references: deferred,
        operations,
        unresolved_mappings: unresolved,
        risks,
        verification_strategy,
        recovery_strategy,
        approval_requirements,
        options: input.options,
        extensions: input.extensions,
    };
    let hash = plan.compute_hash();
    Ok((plan, hash))
}

fn select_objects<'a>(s: &SystemSchema, names: impl Iterator<Item = &'a str>) -> SystemSchema {
    let names: BTreeSet<&str> = names.collect();
    SystemSchema {
        objects: s.objects.iter().filter(|o| names.contains(o.name.as_str())).cloned().collect(),
        relationships: s
            .relationships
            .iter()
            .filter(|r| names.contains(r.from_object.as_str()) && names.contains(r.to_object.as_str()))
            .cloned()
            .collect(),
    }
}

/// Topological order over `Ref` dependencies (referenced objects first). Cycles are broken by
/// deferring reference fields to a second pass; self-references are always deferred.
pub fn order_objects(mapping: &mut MappingSpec) -> (Vec<String>, Vec<DeferredReference>) {
    let names: Vec<String> = mapping.objects.iter().map(|o| o.source_object.clone()).collect();
    let mut deferred = vec![];
    // self references
    for om in &mut mapping.objects {
        let me = om.source_object.clone();
        for fm in &mut om.fields {
            let mut refs = vec![];
            fm.expr.referenced_objects(&mut refs);
            if refs.contains(&me) {
                fm.deferred = true;
                deferred.push(DeferredReference { source_object: me.clone(), target_field: fm.target.clone(), references: me.clone() });
            }
        }
    }
    loop {
        // edges: object -> objects it depends on (non-deferred refs)
        let deps: BTreeMap<String, BTreeSet<String>> = mapping
            .objects
            .iter()
            .map(|om| {
                let mut d = BTreeSet::new();
                for fm in om.fields.iter().filter(|f| !f.deferred && f.status != ReviewStatus::Rejected) {
                    let mut refs = vec![];
                    fm.expr.referenced_objects(&mut refs);
                    for r in refs {
                        if r != om.source_object && names.contains(&r) {
                            d.insert(r);
                        }
                    }
                }
                (om.source_object.clone(), d)
            })
            .collect();
        let mut order = vec![];
        let mut done = BTreeSet::new();
        loop {
            let ready: Vec<String> =
                names.iter().filter(|n| !done.contains(*n) && deps[*n].iter().all(|d| done.contains(d))).cloned().collect();
            if ready.is_empty() {
                break;
            }
            for r in ready {
                done.insert(r.clone());
                order.push(r);
            }
        }
        if order.len() == names.len() {
            return (order, deferred);
        }
        // cycle: defer one reference among the stuck objects (deterministic choice)
        let stuck: Vec<&String> = names.iter().filter(|n| !done.contains(*n)).collect();
        // `to` reaches `from` through dependencies among stuck objects → edge from→to is on a cycle
        let reaches = |start: &str, goal: &str| -> bool {
            let mut stack = vec![start.to_string()];
            let mut seen = BTreeSet::new();
            while let Some(n) = stack.pop() {
                if n == goal {
                    return true;
                }
                if seen.insert(n.clone()) {
                    stack.extend(deps[&n].iter().filter(|d| !done.contains(*d)).cloned());
                }
            }
            false
        };
        let mut broke = false;
        'outer: for s in &stuck {
            let om = mapping.objects.iter_mut().find(|o| &&o.source_object == s).expect("exists");
            for fm in om.fields.iter_mut().filter(|f| !f.deferred) {
                let mut refs = vec![];
                fm.expr.referenced_objects(&mut refs);
                if let Some(r) = refs.iter().find(|r| stuck.contains(r) && *r != *s && reaches(r, s)) {
                    fm.deferred = true;
                    deferred.push(DeferredReference {
                        source_object: om.source_object.clone(),
                        target_field: fm.target.clone(),
                        references: r.clone(),
                    });
                    broke = true;
                    break 'outer;
                }
            }
        }
        if !broke {
            // should not happen; fall back to declaration order
            return (names, deferred);
        }
    }
}

fn expr_summary(e: &Expr) -> String {
    let mut p = vec![];
    e.source_paths(&mut p);
    let op = serde_json::to_value(e).ok().and_then(|v| v.get("op").and_then(|o| o.as_str()).map(String::from)).unwrap_or_default();
    format!("{op}({})", p.join(", "))
}

/// Human-readable plan (Markdown) shown to the operator before approval.
pub fn render_markdown(plan: &MigrationPlan, hash: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Migration plan `{}`\n\n", plan.plan_id));
    s.push_str(&format!("* **Plan hash (SHA-256):** `{hash}`\n"));
    s.push_str(&format!(
        "* **Source:** `{}` — {} ({}), connector `{}@{}`{}\n",
        plan.source.connection,
        plan.source.identity.name,
        plan.source.identity.kind,
        plan.source.connector.id,
        plan.source.connector.version,
        if plan.source.connector.generated { " (generated)" } else { "" }
    ));
    s.push_str(&format!(
        "* **Destination:** `{}` — {} ({}), connector `{}@{}`{}, environment **{}**\n",
        plan.destination.connection,
        plan.destination.identity.name,
        plan.destination.identity.kind,
        plan.destination.connector.id,
        plan.destination.connector.version,
        if plan.destination.connector.generated { " (generated)" } else { "" },
        plan.destination.environment
    ));
    s.push_str(&format!("* **Operations:** {}\n", plan.operations.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", ")));
    s.push_str(&format!("* **Execution order:** {}\n", plan.execution_order.join(" → ")));
    if !plan.deferred_references.is_empty() {
        s.push_str(&format!(
            "* **Second pass (cyclic references):** {}\n",
            plan.deferred_references
                .iter()
                .map(|d| format!("{}.{} → {}", d.source_object, d.target_field, d.references))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    s.push_str("\n## Objects\n\n");
    for o in &plan.execution_order {
        let Some(om) = plan.mapping.object(o) else { continue };
        let est = plan.estimated_records.get(o).copied().flatten().map(|n| n.to_string()).unwrap_or_else(|| "unknown".into());
        s.push_str(&format!(
            "### {} → {} (≈{est} records, {:?}{})\n\n",
            om.source_object,
            om.target_object,
            om.write_mode,
            om.target_key_field.as_ref().map(|k| format!(", key `{k}`")).unwrap_or_default()
        ));
        s.push_str("| destination field | source / transform | confidence | status |\n|---|---|---|---|\n");
        for f in &om.fields {
            s.push_str(&format!(
                "| `{}` | {} | {:.2} | {:?}{} |\n",
                f.target,
                expr_summary(&f.expr),
                f.confidence,
                f.status,
                if f.deferred { " (second pass)" } else { "" }
            ));
        }
        s.push('\n');
    }
    s.push_str("## Risks\n\n");
    if plan.risks.is_empty() {
        s.push_str("None identified.\n");
    }
    for r in &plan.risks {
        s.push_str(&format!(
            "* **{:?}** `{}`{}: {}\n",
            r.severity,
            r.code,
            r.object.as_ref().map(|o| format!(" ({o})")).unwrap_or_default(),
            r.message
        ));
    }
    if !plan.unresolved_mappings.is_empty() {
        s.push_str("\n## Mappings needing review\n\n");
        for u in &plan.unresolved_mappings {
            s.push_str(&format!("* {u}\n"));
        }
    }
    s.push_str("\n## Verification\n\n");
    for v in &plan.verification_strategy {
        s.push_str(&format!("* {v}\n"));
    }
    s.push_str("\n## Recovery\n\n");
    for v in &plan.recovery_strategy {
        s.push_str(&format!("* {v}\n"));
    }
    s.push_str("\n## Approval requirements\n\n");
    for v in &plan.approval_requirements {
        s.push_str(&format!("* {v}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn om(src: &str, refs: &[(&str, &str)]) -> ObjectMapping {
        ObjectMapping {
            source_object: src.into(),
            target_object: src.into(),
            target_key_field: None,
            fields: refs
                .iter()
                .map(|(f, o)| FieldMapping::accepted(*f, Expr::Ref { input: Box::new(Expr::field(*f)), object: (*o).into() }))
                .collect(),
            unmapped_fields: UnmappedFieldPolicy::Archive,
            skip_fields: vec![],
            write_mode: WriteMode::Insert,
        }
    }

    #[test]
    fn orders_parents_first_and_breaks_cycles() {
        let mut m = MappingSpec {
            mapping_version: 1,
            objects: vec![
                om("activities", &[("contact_id", "contacts"), ("org_id", "orgs")]),
                om("contacts", &[("org_id", "orgs"), ("manager_id", "contacts")]),
                om("orgs", &[("primary_contact", "contacts")]),
            ],
        };
        let (order, deferred) = order_objects(&mut m);
        assert_eq!(order.len(), 3);
        assert_eq!(order.last().unwrap(), "activities");
        // self reference deferred + the orgs<->contacts cycle broken once
        assert!(deferred.iter().any(|d| d.source_object == "contacts" && d.target_field == "manager_id"));
        assert_eq!(deferred.len(), 2);
        let pos = |n: &str| order.iter().position(|o| o == n).unwrap();
        // whichever edge was deferred, the remaining dependency is respected
        let contacts_org_deferred = deferred.iter().any(|d| d.source_object == "contacts" && d.target_field == "org_id");
        if contacts_org_deferred { assert!(pos("contacts") < pos("orgs")) } else { assert!(pos("orgs") < pos("contacts")) }
    }
}
