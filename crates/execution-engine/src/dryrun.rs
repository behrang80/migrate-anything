//! Dry run: read-only source sampling, full transformation, destination-requirement checks.
//! Never writes to the destination (`wrote_to_destination` is always false and the sink is
//! never touched).

use std::collections::BTreeSet;

use ma_connector_sdk::ConnectorHandle;
use ma_mapping::ExtensionRunner;
use ma_types::*;

use crate::transform::BatchTransformer;

pub async fn dry_run(
    plan: &MigrationPlan,
    plan_hash: &str,
    source: &ConnectorHandle,
    extensions: Option<&dyn ExtensionRunner>,
    sample_size: usize,
) -> MaResult<DryRunReport> {
    let src = source.source()?;
    let mut objects = vec![];
    let mut issues: Vec<Risk> = plan.risks.iter().filter(|r| r.severity == Severity::Blocking).cloned().collect();
    for name in &plan.execution_order {
        let Some(om) = plan.mapping.object(name) else { continue };
        let target = plan.destination_schema.object(&om.target_object);
        let batch = src.read_batch(name, None, sample_size.max(1)).await?;
        let t = BatchTransformer::new(om, target, extensions);
        // references cannot be resolved before anything is migrated: they are checked
        // structurally (present / null) instead.
        let out = t.transform(&batch.records, None, true).await?;
        let mut r = DryRunObjectResult { source_object: name.clone(), target_object: om.target_object.clone(), ..Default::default() };
        let mut keys = BTreeSet::new();
        let mut archived = BTreeSet::new();
        for (rec, tr) in batch.records.iter().zip(out.iter()) {
            r.sampled += 1;
            if tr.errors.is_empty() && tr.unsupported.is_empty() {
                r.transform_ok += 1;
            } else {
                r.transform_failed += 1;
                for e in tr.errors.iter().chain(tr.unsupported.iter()) {
                    if let Some(rest) = e.strip_prefix("required destination field `") {
                        let f = rest.split('`').next().unwrap_or_default().to_string();
                        *r.missing_required.entry(f).or_default() += 1;
                    } else if let Some(rest) = e.strip_prefix("field `") {
                        let f = rest.split('`').next().unwrap_or_default().to_string();
                        *r.invalid_values.entry(f).or_default() += 1;
                    }
                }
                if r.examples.len() < 5 {
                    r.examples.push(format!(
                        "{} {}: {}",
                        name,
                        rec.id,
                        tr.errors.iter().chain(tr.unsupported.iter()).cloned().collect::<Vec<_>>().join("; ")
                    ));
                }
            }
            r.unresolved_reference_checks +=
                tr.fields.values().filter(|v| matches!(v, Value::String(s) if s.starts_with("<ref:"))).count() as u64;
            if let Some(k) = om.target_key_field.as_ref().and_then(|k| tr.fields.get(k)).and_then(|v| v.to_text())
                && !keys.insert(k)
            {
                r.duplicate_keys_in_sample += 1;
            }
            for (f, _, _) in &tr.archived {
                archived.insert(f.clone());
            }
        }
        r.archived_fields = archived.into_iter().collect();
        if r.transform_failed > 0 {
            issues.push(Risk {
                severity: Severity::Warning,
                code: "sample_transform_failures".into(),
                message: format!("{} of {} sampled records would fail: {}", r.transform_failed, r.sampled, r.examples.join(" | ")),
                object: Some(name.clone()),
            });
        }
        if r.duplicate_keys_in_sample > 0 {
            issues.push(Risk {
                severity: Severity::Warning,
                code: "duplicate_keys".into(),
                message: format!("{} duplicate external keys in the sample; upserts would merge these records", r.duplicate_keys_in_sample),
                object: Some(name.clone()),
            });
        }
        objects.push(r);
    }
    let passed = !issues.iter().any(|i| i.severity == Severity::Blocking) && objects.iter().all(|o| o.transform_failed == 0);
    Ok(DryRunReport { plan_hash: plan_hash.to_string(), wrote_to_destination: false, objects, issues, passed })
}
