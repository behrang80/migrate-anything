//! Verification of actual migration results and the evidence-based final report.
//!
//! Everything is streamed: the source is re-read once (to recompute the expected destination
//! content of every written record) and the destination is read once. Key coverage, duplicates
//! and reference integrity are computed in SQLite scratch tables, so memory stays bounded.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ma_connector_sdk::ConnectorHandle;
use ma_execution::BatchTransformer;
use ma_mapping::ExtensionRunner;
use ma_persistence::Store;
use ma_types::*;
use sha2::{Digest, Sha256};

pub struct Verifier<'a> {
    pub store: &'a Store,
    pub plan: &'a MigrationPlan,
    pub source: &'a ConnectorHandle,
    pub destination: &'a ConnectorHandle,
    pub extensions: Option<Arc<dyn ExtensionRunner>>,
    pub job_id: &'a str,
}

const PAGE: usize = 1000;

/// Canonical comparison text for a value (null/missing are excluded by callers).
pub fn canonical(v: &Value, ty: Option<FieldType>) -> String {
    // A destination text field is compared as text, exactly: "1.50" and "1.5" are different.
    if ty == Some(FieldType::String) {
        return v.to_text().unwrap_or_default();
    }
    let v = match (v, ty) {
        (Value::String(s), Some(t)) if t != FieldType::String => Value::parse_as(s, t),
        (Value::Int(_) | Value::Float(_) | Value::Decimal(_), Some(FieldType::String)) => v.clone(),
        _ => v.clone(),
    };
    match &v {
        Value::Int(i) => i.to_string(),
        Value::Decimal(d) => trim_decimal(d),
        Value::Float(f) => trim_decimal(&format!("{f}")),
        Value::Bool(b) => b.to_string(),
        Value::Date(d) => d.format("%Y-%m-%d").to_string(),
        Value::DateTime(dt) => dt.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
        Value::Array(items) => format!("[{}]", items.iter().map(|i| canonical(i, None)).collect::<Vec<_>>().join(",")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| format!("{k}:{}", canonical(v, None))).collect::<Vec<_>>().join(",")
        ),
        Value::String(s) if ty.is_none() => {
            // untyped text from a schemaless destination: compare by inferred meaning
            match Value::infer_from_text(s) {
                Value::String(t) => t,
                inferred => canonical(&inferred, None),
            }
        }
        Value::String(s) => {
            // numeric-looking strings compare numerically (CSV/JSON round trips)
            if s.parse::<i64>().is_ok() && !(s.len() > 1 && s.starts_with('0')) || is_decimal_text(s) { trim_decimal(s) } else { s.clone() }
        }
        other => other.to_text().unwrap_or_default(),
    }
}

fn trim_decimal(s: &str) -> String {
    let s = normalize_decimal(s);
    let out = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if out == "-0" { "0".into() } else { out }
}

fn record_digest(key: &str, fields: &BTreeMap<String, String>) -> u64 {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    for (k, v) in fields {
        h.update([0u8]);
        h.update(k.as_bytes());
        h.update([1u8]);
        h.update(v.as_bytes());
    }
    let d = h.finalize();
    u64::from_le_bytes(d[..8].try_into().expect("8 bytes"))
}

fn projection(fields: &BTreeMap<String, Value>, wanted: &[(String, Option<FieldType>)]) -> BTreeMap<String, String> {
    wanted
        .iter()
        .filter_map(|(f, ty)| match fields.get(f) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(v) => Some((f.clone(), canonical(v, *ty))),
        })
        .collect()
}

impl Verifier<'_> {
    pub async fn verify(&self) -> MaResult<VerificationResult> {
        self.store.verify_reset(self.job_id).await?;
        let mut objects = vec![];
        let mut checks = vec![];
        let mut discrepancies = vec![];
        let src = self.source.source()?;
        let dst =
            self.destination.source().map_err(|_| MaError::unsupported("destination cannot be read back; verification impossible"))?;
        let job = self.store.get_job(self.job_id).await?;

        for name in &self.plan.execution_order {
            let om = self.plan.mapping.object(name).ok_or_else(|| MaError::internal("mapping"))?;
            let target = self.plan.destination_schema.object(&om.target_object);
            let stage = job.stages.iter().find(|s| &s.source_object == name).map(|s| s.counters.clone()).unwrap_or_default();
            let mut ov = ObjectVerification { source_object: name.clone(), target_object: om.target_object.clone(), ..Default::default() };
            // field types as written by the sink (destination schema, or derived from the mapping
            // for destinations that create objects on write)
            let spec = ma_execution::sink_spec(self.plan, om);
            let wanted: Vec<(String, Option<FieldType>)> = om
                .fields
                .iter()
                .filter(|f| f.status != ReviewStatus::Rejected)
                .map(|f| {
                    let ty = target
                        .and_then(|t| t.field(&f.target))
                        .map(|d| d.field_type)
                        .or_else(|| spec.fields.iter().find(|d| d.name == f.target).map(|d| d.field_type))
                        .filter(|t| *t != FieldType::Unknown);
                    (f.target.clone(), ty)
                })
                .collect();
            let ref_fields: Vec<(String, String)> = om
                .fields
                .iter()
                .filter(|f| f.status != ReviewStatus::Rejected)
                .filter_map(|f| {
                    let mut r = vec![];
                    f.expr.referenced_objects(&mut r);
                    r.first().map(|o| (f.target.clone(), o.clone()))
                })
                .collect();
            let sample_n = self.plan.options.verify_sample_size;

            // ---- pass A: source → expected content of written records
            let transformer = BatchTransformer::new(om, target, self.extensions.as_deref());
            let mut expected_sum = 0u64;
            let mut expected_n = 0u64;
            let mut sample: BTreeMap<u64, (String, BTreeMap<String, String>)> = BTreeMap::new();
            let mut cursor: Option<Cursor> = None;
            loop {
                let b = src.read_batch(name, cursor.as_ref(), PAGE).await?;
                ov.source_count += b.records.len() as u64;
                let ids: Vec<String> = b.records.iter().map(|r| r.id.clone()).collect();
                let written: HashMap<String, String> = self.store.lookup_dest_ids(self.job_id, name, &ids).await?.into_iter().collect();
                let recs: Vec<Record> = b.records.into_iter().filter(|r| written.contains_key(&r.id)).collect();
                let out = transformer.transform(&recs, Some((self.store, self.job_id)), false).await?;
                for (r, t) in recs.iter().zip(out) {
                    let key = om
                        .target_key_field
                        .as_ref()
                        .and_then(|k| t.fields.get(k))
                        .and_then(|v| v.to_text())
                        .unwrap_or_else(|| written[&r.id].clone());
                    let proj = projection(&t.fields, &wanted);
                    expected_sum = expected_sum.wrapping_add(record_digest(&key, &proj));
                    expected_n += 1;
                    let rank = record_digest(&r.id, &BTreeMap::new());
                    if sample.len() < sample_n || sample.keys().next_back().is_some_and(|m| rank < *m) {
                        sample.insert(rank, (key, proj));
                        if sample.len() > sample_n {
                            let last = *sample.keys().next_back().expect("non-empty");
                            sample.remove(&last);
                        }
                    }
                }
                match b.next_cursor {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            let sample_by_key: HashMap<String, BTreeMap<String, String>> = sample.into_values().collect();

            // ---- pass B: destination
            let mut actual_sum = 0u64;
            let mut actual_n = 0u64;
            let mut dest_rows = 0u64;
            let mut mismatches: Vec<String> = vec![];
            let mut cursor: Option<Cursor> = None;
            loop {
                let b = dst.read_batch(&om.target_object, cursor.as_ref(), PAGE).await?;
                dest_rows += b.records.len() as u64;
                let keyed: Vec<(String, &Record)> = b
                    .records
                    .iter()
                    .map(|r| {
                        let k = om
                            .target_key_field
                            .as_ref()
                            .and_then(|k| r.fields.get(k))
                            .and_then(|v| v.to_text())
                            .unwrap_or_else(|| r.id.clone());
                        (k, r)
                    })
                    .collect();
                let keys: Vec<String> = keyed.iter().map(|(k, _)| k.clone()).collect();
                let ours: HashMap<String, String> = self.store.keys_in_id_map(self.job_id, name, &keys).await?.into_iter().collect();
                let mine: Vec<&(String, &Record)> = keyed.iter().filter(|(k, _)| ours.contains_key(k)).collect();
                self.store
                    .verify_add_keys(self.job_id, &om.target_object, &mine.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>())
                    .await?;
                let mut refs = vec![];
                for (k, r) in &mine {
                    let proj = projection(&r.fields, &wanted);
                    actual_sum = actual_sum.wrapping_add(record_digest(k, &proj));
                    actual_n += 1;
                    for (f, obj) in &ref_fields {
                        if let Some(v) = r.fields.get(f).filter(|v| !v.is_null()).and_then(|v| v.to_text())
                            && !v.is_empty()
                        {
                            refs.push((f.clone(), obj.clone(), v));
                        }
                    }
                    if let Some(exp) = sample_by_key.get(k) {
                        ov.sampled += 1;
                        let diffs: Vec<String> = wanted
                            .iter()
                            .map(|(f, _)| f)
                            .filter(|f| exp.get(*f) != proj.get(*f))
                            .map(|f| format!("{f}: expected {:?}, found {:?}", exp.get(f), proj.get(f)))
                            .collect();
                        if !diffs.is_empty() {
                            ov.sample_mismatches += 1;
                            if mismatches.len() < 5 {
                                mismatches.push(format!("{}[{k}] {}", om.target_object, diffs.join("; ")));
                            }
                        }
                    }
                }
                self.store.verify_add_refs(self.job_id, &om.target_object, &refs).await?;
                match b.next_cursor {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }

            let (seen, distinct, found, idmap) = self.store.verify_key_stats(self.job_id, &om.target_object, name).await?;
            let _ = (seen, distinct);
            ov.destination_count = Some(dest_rows);
            ov.written = stage.written();
            ov.eligible = ov.source_count.saturating_sub(stage.outcome(OutcomeKind::SkippedIntentionally));
            ov.keys_verified = found;
            ov.keys_missing = idmap.saturating_sub(found);
            ov.duplicate_keys = self.store.verify_duplicate_keys(self.job_id, &om.target_object, name).await?;
            let (rc, rb) = self.store.verify_ref_stats(self.job_id, &om.target_object).await?;
            ov.references_checked = rc;
            ov.references_broken = rb;
            ov.checksum_match = Some(expected_sum == actual_sum && expected_n == actual_n);

            let o = Some(name.clone());
            let mut check = |n: &str, passed: bool, expected: String, actual: String, detail: String| {
                if !passed {
                    discrepancies.push(format!("{name}: {n} — {detail}"));
                }
                checks.push(CheckResult {
                    name: n.into(),
                    object: o.clone(),
                    passed,
                    expected: Some(expected),
                    actual: Some(actual),
                    detail,
                });
            };
            check(
                "written_matches_id_map",
                idmap == ov.written,
                ov.written.to_string(),
                idmap.to_string(),
                "every successful write has an id-map entry".into(),
            );
            let missing_examples = if ov.keys_missing > 0 {
                self.store.verify_missing_examples(self.job_id, &om.target_object, name, 5).await?.join(", ")
            } else {
                String::new()
            };
            check(
                "identifier_coverage",
                ov.keys_missing == 0,
                idmap.to_string(),
                found.to_string(),
                if ov.keys_missing == 0 {
                    "all written records found in destination by key".into()
                } else {
                    format!("{} written records not found in destination (e.g. source ids {missing_examples})", ov.keys_missing)
                },
            );
            check(
                "no_duplicates",
                ov.duplicate_keys == 0,
                "0".into(),
                ov.duplicate_keys.to_string(),
                "destination keys of migrated records are unique".into(),
            );
            check(
                "referential_integrity",
                ov.references_broken == 0,
                "0 broken".into(),
                format!("{} broken of {}", ov.references_broken, ov.references_checked),
                "every migrated reference points at a migrated destination record".into(),
            );
            check(
                "sampled_field_comparison",
                ov.sample_mismatches == 0,
                format!("{} matching", ov.sampled),
                format!("{} mismatching", ov.sample_mismatches),
                if mismatches.is_empty() { "sampled records match field-by-field".into() } else { mismatches.join(" | ") },
            );
            check(
                "content_checksum",
                ov.checksum_match == Some(true),
                format!("{expected_n} records / {expected_sum:016x}"),
                format!("{actual_n} records / {actual_sum:016x}"),
                "order-independent checksum over all mapped fields of all written records".into(),
            );
            objects.push(ov);
        }
        let result =
            VerificationResult { job_id: self.job_id.to_string(), verified_at: chrono::Utc::now(), objects, checks, discrepancies };
        self.store.put_validation(&result).await?;
        self.store.verify_reset(self.job_id).await?;
        Ok(result)
    }
}

/// Build the final report strictly from stored evidence.
pub async fn build_report(store: &Store, plan: &MigrationPlan, plan_hash: &str, job_id: &str) -> MaResult<MigrationReport> {
    let job = store.get_job(job_id).await?;
    let verification = store.get_validation(job_id).await?;
    let mut outcomes: BTreeMap<String, u64> = BTreeMap::new();
    let (mut attempted, mut written, mut skipped, mut failed, mut review, mut archived, mut second_pass_failed) = (0, 0, 0, 0, 0, 0, 0);
    for s in &job.stages {
        if s.source_object.ends_with("#pass2") {
            second_pass_failed += s.counters.outcome(OutcomeKind::Failed);
            continue;
        }
        for (k, v) in &s.counters.outcomes {
            *outcomes.entry(k.clone()).or_default() += v;
        }
        attempted += s.counters.attempted;
        written += s.counters.written();
        skipped += s.counters.outcome(OutcomeKind::SkippedIntentionally);
        failed += s.counters.outcome(OutcomeKind::Failed) + s.counters.outcome(OutcomeKind::Unsupported);
        review += s.counters.outcome(OutcomeKind::RequiresReconciliation);
        archived += s.counters.archived_values;
    }
    review += second_pass_failed;
    let mut unsupported_fields = vec![];
    for (obj, kind, field, n) in store.exception_counts(job_id).await? {
        if kind == "archived_field"
            && let Some(f) = field
        {
            unsupported_fields.push(format!("{obj}.{f} ({n} values archived)"));
        }
    }
    let read_total: u64 = job.stages.iter().filter(|s| !s.source_object.ends_with("#pass2")).map(|s| s.counters.read).sum();
    let (discovered, verified_n, rel_checked, rel_ok, discrepancies, verified) = match &verification {
        Some(v) => (
            v.objects.iter().map(|o| o.source_count).sum::<u64>(),
            v.objects.iter().map(|o| o.keys_verified).sum::<u64>(),
            v.objects.iter().map(|o| o.references_checked).sum::<u64>(),
            v.objects.iter().map(|o| o.references_checked - o.references_broken).sum::<u64>(),
            v.discrepancies.clone(),
            true,
        ),
        None => (read_total, 0, 0, 0, vec![], false),
    };
    let mut discrepancies = discrepancies;
    if second_pass_failed > 0 {
        discrepancies.push(format!("{second_pass_failed} deferred references could not be written in the second pass"));
    }
    let final_status = match job.status {
        JobStatus::Cancelled => FinalStatus::Cancelled,
        JobStatus::Failed if written == 0 => FinalStatus::Failed,
        JobStatus::Failed | JobStatus::Running | JobStatus::Paused | JobStatus::Pending => FinalStatus::PartiallyCompleted,
        JobStatus::Completed if !verified => FinalStatus::NotVerified,
        JobStatus::Completed => {
            let missing: u64 = verification.as_ref().map(|v| v.objects.iter().map(|o| o.keys_missing).sum()).unwrap_or(0);
            if missing > 0 {
                FinalStatus::PartiallyCompleted
            } else if discrepancies.is_empty() && failed == 0 && review == 0 && archived == 0 && skipped == 0 {
                FinalStatus::CompletedAndVerified
            } else {
                FinalStatus::CompletedWithExceptions
            }
        }
    };
    Ok(MigrationReport {
        source_system: format!("{} ({}, {})", plan.source.identity.name, plan.source.identity.kind, plan.source.connection),
        destination_system: format!(
            "{} ({}, {})",
            plan.destination.identity.name, plan.destination.identity.kind, plan.destination.connection
        ),
        plan_id: plan.plan_id.clone(),
        plan_hash: plan_hash.to_string(),
        job_id: job_id.to_string(),
        execution_start: job.started_at,
        execution_end: job.finished_at,
        records_discovered: discovered,
        records_eligible: discovered.saturating_sub(skipped),
        records_attempted: attempted,
        records_written: written,
        records_verified: verified_n,
        records_skipped: skipped,
        records_failed: failed,
        records_requiring_review: review,
        outcomes,
        relationships_checked: rel_checked,
        relationships_preserved: rel_ok,
        attachments_migrated: 0,
        archived_values: archived,
        unsupported_fields,
        unresolved_discrepancies: discrepancies,
        final_status,
        verified,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_normalizes_representations() {
        assert_eq!(canonical(&Value::Decimal("12.50".into()), Some(FieldType::Decimal)), "12.5");
        assert_eq!(canonical(&Value::String("12.5".into()), Some(FieldType::Decimal)), "12.5");
        assert_eq!(canonical(&Value::Int(12), Some(FieldType::Decimal)), "12");
        assert_eq!(canonical(&Value::String("007".into()), Some(FieldType::String)), "007");
        assert_ne!(
            canonical(&Value::String("1.50".into()), Some(FieldType::String)),
            canonical(&Value::String("1.5".into()), Some(FieldType::String)),
            "text fields must not be normalized numerically"
        );
        assert_eq!(canonical(&Value::Decimal("1.50".into()), Some(FieldType::String)), "1.50");
        let a = canonical(&Value::String("2024-01-02T10:00:00+02:00".into()), Some(FieldType::DateTime));
        let b = canonical(&Value::String("2024-01-02 08:00:00".into()), Some(FieldType::DateTime));
        assert_eq!(a, b);
    }
}
