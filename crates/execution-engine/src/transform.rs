//! Batch transformation with id-map reference resolution (shared by execution, dry run and
//! verification).

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use ma_mapping::{EvalContext, ExtensionRunner, TransformedRecord, runtime::consumed_fields, transform_record};
use ma_persistence::Store;
use ma_types::*;

pub struct BatchTransformer<'a> {
    pub om: &'a ObjectMapping,
    pub target: Option<&'a ObjectDef>,
    pub consumed: BTreeSet<String>,
    pub extensions: Option<&'a dyn ExtensionRunner>,
    pub has_refs: bool,
}

impl<'a> BatchTransformer<'a> {
    pub fn new(om: &'a ObjectMapping, target: Option<&'a ObjectDef>, extensions: Option<&'a dyn ExtensionRunner>) -> Self {
        let has_refs = !om.referenced_objects().is_empty();
        BatchTransformer { om, target, consumed: consumed_fields(om), extensions, has_refs }
    }

    /// Transform records; references are resolved through the job's id map (`job_id`), or left
    /// unresolved (`None`, dry run).
    pub async fn transform(
        &self,
        records: &[Record],
        store: Option<(&Store, &str)>,
        skip_deferred: bool,
    ) -> MaResult<Vec<TransformedRecord>> {
        let mut resolved: HashMap<(String, String), String> = HashMap::new();
        if self.has_refs
            && let Some((store, job)) = store
        {
            // pass 1: discover which references are needed
            let wanted = {
                let wanted: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());
                let recorder = |o: &str, id: &str| {
                    wanted.lock().expect("lock").insert((o.to_string(), id.to_string()));
                    None
                };
                let ctx = EvalContext { refs: &recorder, extensions: None };
                for r in records {
                    let _ = transform_record(self.om, None, &self.consumed, r, &ctx, skip_deferred);
                }
                wanted.into_inner().expect("lock")
            };
            let mut by_object: HashMap<String, Vec<String>> = HashMap::new();
            for (o, id) in wanted {
                by_object.entry(o).or_default().push(id);
            }
            for (o, ids) in by_object {
                for (sid, did) in store.lookup_dest_ids(job, &o, &ids).await? {
                    resolved.insert((o.clone(), sid), did);
                }
            }
        }
        let resolver = |o: &str, id: &str| resolved.get(&(o.to_string(), id.to_string())).cloned();
        let dry = |_: &str, id: &str| Some(format!("<ref:{id}>"));
        let ctx = if store.is_some() {
            EvalContext { refs: &resolver, extensions: self.extensions }
        } else {
            EvalContext { refs: &dry, extensions: self.extensions }
        };
        Ok(records.iter().map(|r| transform_record(self.om, self.target, &self.consumed, r, &ctx, skip_deferred)).collect())
    }
}
