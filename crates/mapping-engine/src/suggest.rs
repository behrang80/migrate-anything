//! Deterministic heuristic mapping suggestions.
//!
//! Scoring uses normalized names, a domain-neutral synonym table, token overlap, string
//! similarity, type compatibility and declared/inferred relationships. Every suggestion carries
//! its evidence. Suggestions at or above `accept_threshold` are `Accepted`; others `NeedsReview`.

use std::collections::{BTreeMap, BTreeSet};

use ma_types::*;

#[derive(Debug, Clone)]
pub struct SuggestOptions {
    pub accept_threshold: f32,
    pub min_confidence: f32,
}

impl Default for SuggestOptions {
    fn default() -> Self {
        SuggestOptions { accept_threshold: 0.9, min_confidence: 0.45 }
    }
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct SuggestionSet {
    pub pairings: Vec<ObjectPairing>,
    pub suggestions: Vec<MappingSuggestion>,
    /// Draft executable mapping built from the suggestions (review statuses applied).
    pub mapping: MappingSpec,
    pub unmatched_source_fields: BTreeMap<String, Vec<String>>,
    pub unmatched_required_targets: BTreeMap<String, Vec<String>>,
    pub needs_review: usize,
}

const SYNONYMS: &[&[&str]] = &[
    &["name", "title", "label", "full name", "display name"],
    &["company", "organization", "organisation", "org", "account", "business", "legal name", "company name", "employer"],
    &["email", "email address", "mail", "e mail"],
    &["phone", "telephone", "tel", "mobile", "phone number"],
    &["external id", "legacy id", "source id", "ext id", "original id", "import id"],
    &["number", "no", "num", "code", "reference", "ref"],
    &["amount", "total", "value", "sum", "gross", "grand total"],
    &["currency", "currency code", "ccy", "iso currency"],
    &["created", "created at", "creation date", "date created", "created on", "opened at", "opened"],
    &["updated", "updated at", "modified", "last modified", "modified at", "changed at"],
    &["address", "street", "billing address", "address line", "street address"],
    &["first name", "given name", "fname", "forename"],
    &["last name", "surname", "family name", "lname"],
    &["description", "notes", "note", "comment", "comments", "body", "details", "memo"],
    &["status", "state", "stage", "phase"],
    &["owner", "assigned to", "assignee", "responsible", "assigned"],
    &["due", "due date", "deadline", "due on"],
    &["quantity", "qty", "count", "units"],
    &["price", "unit price", "rate", "unit cost"],
    &["subject", "topic", "headline"],
    &["type", "kind", "category", "class"],
    &["contact", "person", "people", "individual"],
    &["opportunity", "deal", "pipeline item"],
    &["activity", "interaction", "event", "task", "touchpoint"],
    &["invoice", "bill"],
    &["line", "item", "line item", "invoice line", "detail line"],
    &["customer", "client", "buyer", "account holder"],
    &["date", "day", "on"],
    &["city", "town", "locality"],
    &["zip", "postal code", "postcode", "zip code"],
    &["country", "country code", "nation"],
    &["website", "url", "web", "homepage", "domain"],
    &["probability", "likelihood", "chance"],
    &["close date", "expected close", "closing date"],
    &["issued", "issue date", "invoice date", "issued on"],
    &["tax", "vat", "tax amount"],
    &["active", "enabled", "is active"],
    &["industry", "sector", "vertical"],
];

/// Split identifiers into lower-case tokens: `billingAddress_line1` → [billing, address, line, 1].
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let boundary =
            i > 0 && ((c.is_uppercase() && chars[i - 1].is_lowercase()) || (c.is_ascii_digit() != chars[i - 1].is_ascii_digit()));
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn singular(t: &str) -> String {
    if let Some(s) = t.strip_suffix("ies") {
        format!("{s}y")
    } else if t.ends_with("sses") || t.ends_with("ches") || t.ends_with("shes") || t.ends_with("xes") {
        t[..t.len() - 2].to_string()
    } else if t.len() > 3 && t.ends_with('s') && !t.ends_with("ss") && !t.ends_with("us") {
        t[..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

const NOISE: &[&str] = &["x", "c", "cf", "custom", "fld", "field", "tbl", "the", "is", "has", "of"];

fn norm(s: &str) -> String {
    tokens(s).iter().filter(|t| !NOISE.contains(&t.as_str())).map(|t| singular(t)).collect::<Vec<_>>().join(" ")
}

fn synonym_group(n: &str) -> Option<usize> {
    SYNONYMS.iter().position(|g| g.iter().any(|w| norm(w) == n))
}

/// Name similarity in [0,1] with an explanation.
pub fn name_score(a: &str, b: &str) -> (f32, String) {
    let (na, nb) = (norm(a), norm(b));
    if na.is_empty() || nb.is_empty() {
        return (0.0, "empty name".into());
    }
    if na == nb {
        return (0.97, format!("names normalize to the same form `{na}`"));
    }
    if let (Some(ga), Some(gb)) = (synonym_group(&na), synonym_group(&nb))
        && ga == gb
    {
        return (0.86, format!("`{na}` and `{nb}` are synonyms"));
    }
    let ta: BTreeSet<String> = na.split(' ').map(String::from).collect();
    let tb: BTreeSet<String> = nb.split(' ').map(String::from).collect();
    let inter = ta.intersection(&tb).count() as f32;
    let union = ta.union(&tb).count() as f32;
    let jacc = inter / union;
    // synonym match on any token pair counts as partial overlap
    let syn_tok = ta.iter().any(|x| tb.iter().any(|y| x != y && synonym_group(x).is_some() && synonym_group(x) == synonym_group(y)));
    let jw = strsim::jaro_winkler(&na, &nb) as f32;
    let mut score = (jacc * 0.85).max(jw * 0.75);
    if syn_tok {
        score = score.max(0.7);
    }
    let why = if jacc > 0.0 {
        format!("token overlap {:.0}% between `{na}` and `{nb}`", jacc * 100.0)
    } else if syn_tok {
        format!("`{na}` and `{nb}` share synonymous tokens")
    } else {
        format!("string similarity {:.2} between `{na}` and `{nb}`", jw)
    };
    (score, why)
}

fn type_adjust(src: FieldType, dst: FieldType) -> (f32, Option<String>) {
    if src == dst || src == FieldType::Unknown || dst == FieldType::Unknown {
        (0.0, None)
    } else if src.converts_to(dst) {
        (-0.05, Some(format!("type conversion {src:?} → {dst:?}")))
    } else {
        (-0.4, Some(format!("incompatible types {src:?} → {dst:?}")))
    }
}

fn is_key_target(name: &str) -> bool {
    let n = norm(name);
    [
        "external id",
        "legacy id",
        "source id",
        "ext id",
        "original id",
        "import id",
        "migration id",
        "legacy ref",
        "source ref",
        "legacy key",
    ]
    .iter()
    .any(|k| n == *k)
}

/// Pair source objects with destination objects.
pub fn pair_objects(src: &SystemSchema, dst: &SystemSchema) -> Vec<ObjectPairing> {
    let mut cands: Vec<(f32, String, String, String)> = vec![];
    for s in &src.objects {
        for d in &dst.objects {
            let (ns, why) = name_score(&s.name, &d.name);
            // field overlap
            let mut hits = 0usize;
            for f in &s.fields {
                if d.fields.iter().any(|g| name_score(&f.name, &g.name).0 >= 0.85) {
                    hits += 1;
                }
            }
            let overlap = if s.fields.is_empty() { 0.0 } else { hits as f32 / s.fields.len().max(d.fields.len()) as f32 };
            let score = (ns * 0.6 + overlap * 0.4).max(ns.min(0.97));
            cands.push((score, s.name.clone(), d.name.clone(), format!("{why}; field overlap {:.0}%", overlap * 100.0)));
        }
    }
    cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let (mut used_s, mut used_d) = (BTreeSet::new(), BTreeSet::new());
    let mut out = vec![];
    for (score, s, d, why) in cands {
        if score < 0.35 || used_s.contains(&s) || used_d.contains(&d) {
            continue;
        }
        used_s.insert(s.clone());
        used_d.insert(d.clone());
        out.push(ObjectPairing { source_object: s, target_object: d, confidence: score.min(1.0), explanation: why });
    }
    out.sort_by(|a, b| a.source_object.cmp(&b.source_object));
    out
}

fn enum_map(src: &FieldDef, dst: &FieldDef) -> Option<(BTreeMap<String, Value>, bool)> {
    if src.enum_values.is_empty() || dst.enum_values.is_empty() {
        return None;
    }
    let mut m = BTreeMap::new();
    let mut complete = true;
    for sv in &src.enum_values {
        let best = dst
            .enum_values
            .iter()
            .map(|dv| (name_score(sv, dv).0, dv))
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        match best {
            Some((s, dv)) if s >= 0.85 => {
                m.insert(sv.clone(), Value::String(dv.clone()));
            }
            _ => complete = false,
        }
    }
    Some((m, complete))
}

pub fn suggest_mapping(
    src: &SystemSchema,
    dst: &SystemSchema,
    explicit_pairs: &[(String, String)],
    opts: &SuggestOptions,
) -> SuggestionSet {
    let pairings: Vec<ObjectPairing> = if explicit_pairs.is_empty() {
        pair_objects(src, dst)
    } else {
        explicit_pairs
            .iter()
            .map(|(s, d)| ObjectPairing {
                source_object: s.clone(),
                target_object: d.clone(),
                confidence: 1.0,
                explanation: "specified explicitly".into(),
            })
            .collect()
    };
    let pair_of: BTreeMap<String, String> = pairings.iter().map(|p| (p.source_object.clone(), p.target_object.clone())).collect();
    let mut suggestions = vec![];
    let mut objects = vec![];
    let mut unmatched_source_fields = BTreeMap::new();
    let mut unmatched_required_targets = BTreeMap::new();

    for p in &pairings {
        let (Some(so), Some(to)) = (src.object(&p.source_object), dst.object(&p.target_object)) else { continue };
        let src_pk = so.primary_key.first().cloned();
        let mut cands: Vec<(f32, MappingSuggestion)> = vec![];
        let mut key_field = None;

        for tf in to.fields.iter().filter(|f| !f.read_only) {
            // external-id style destination fields receive the source identifier
            if is_key_target(&tf.name)
                && let Some(pk) = &src_pk
            {
                let expr = if tf.field_type == FieldType::String {
                    Expr::Cast { input: Box::new(Expr::field(pk)), to: FieldType::String }
                } else {
                    Expr::field(pk)
                };
                cands.push((
                    0.95,
                    MappingSuggestion {
                        source_object: so.name.clone(),
                        target_object: to.name.clone(),
                        source_field: Some(pk.clone()),
                        target_field: tf.name.clone(),
                        expr,
                        confidence: 0.95,
                        explanation: format!("`{}` looks like an external-id field; it will hold the source primary key `{pk}`", tf.name),
                        evidence: vec![
                            "destination field name matches external-id convention".into(),
                            format!("source primary key is `{pk}`"),
                        ],
                        suggested_by: "heuristic".into(),
                    },
                ));
                key_field.get_or_insert(tf.name.clone());
                continue;
            }
            for sf in &so.fields {
                // relationship-aware suggestion
                let src_rel = src.relationships.iter().find(|r| r.from_object == so.name && r.from_field == sf.name);
                let dst_rel = dst.relationships.iter().find(|r| r.from_object == to.name && r.from_field == tf.name);
                if let (Some(sr), Some(dr)) = (src_rel, dst_rel)
                    && pair_of.get(&sr.to_object) == Some(&dr.to_object)
                {
                    let conf = 0.93;
                    cands.push((
                        conf,
                        MappingSuggestion {
                            source_object: so.name.clone(),
                            target_object: to.name.clone(),
                            source_field: Some(sf.name.clone()),
                            target_field: tf.name.clone(),
                            expr: Expr::Ref { input: Box::new(Expr::field(&sf.name)), object: sr.to_object.clone() },
                            confidence: conf,
                            explanation: format!(
                                "`{}` references `{}` and `{}` references `{}`, which are paired; ids are translated through the job's id map",
                                sf.name, sr.to_object, tf.name, dr.to_object
                            ),
                            evidence: vec![format!("source relationship ({:?})", sr.evidence), format!("destination relationship ({:?})", dr.evidence)],
                            suggested_by: "heuristic".into(),
                        },
                    ));
                    continue;
                }
                if src_rel.is_some() != dst_rel.is_some() && (src_rel.is_some() || dst_rel.is_some()) {
                    // a reference on one side only should not be copied as a plain value
                    continue;
                }
                let (ns, why) = name_score(&sf.name, &tf.name);
                let (adj, tnote) = type_adjust(sf.field_type, tf.field_type);
                let mut conf = (ns + adj).clamp(0.0, 1.0);
                let mut evidence = vec![why.clone()];
                let mut expr =
                    if sf.field_type != tf.field_type && tf.field_type != FieldType::Unknown && sf.field_type != FieldType::Unknown {
                        Expr::Cast { input: Box::new(Expr::field(&sf.name)), to: tf.field_type }
                    } else {
                        Expr::field(&sf.name)
                    };
                if let Some(t) = tnote {
                    evidence.push(t);
                }
                if let Some((map, complete)) = enum_map(sf, tf) {
                    evidence.push(format!("enum values mapped {} of {}", map.len(), sf.enum_values.len()));
                    if !complete {
                        conf = conf.min(opts.accept_threshold - 0.05);
                    }
                    expr = Expr::EnumMap { input: Box::new(Expr::field(&sf.name)), map, on_unmapped: Unmapped::Fail };
                }
                if conf >= opts.min_confidence {
                    cands.push((
                        conf,
                        MappingSuggestion {
                            source_object: so.name.clone(),
                            target_object: to.name.clone(),
                            source_field: Some(sf.name.clone()),
                            target_field: tf.name.clone(),
                            expr,
                            confidence: conf,
                            explanation: why,
                            evidence,
                            suggested_by: "heuristic".into(),
                        },
                    ));
                }
            }
        }
        // greedy one-to-one assignment
        cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.target_field.cmp(&b.1.target_field)));
        let (mut used_s, mut used_t) = (BTreeSet::new(), BTreeSet::new());
        let mut chosen = vec![];
        for (_, s) in cands {
            let sfield = s.source_field.clone().unwrap_or_default();
            let is_key = Some(&s.target_field) == key_field.as_ref();
            if used_t.contains(&s.target_field) || (!is_key && used_s.contains(&sfield)) {
                continue;
            }
            used_t.insert(s.target_field.clone());
            if !is_key {
                used_s.insert(sfield);
            }
            chosen.push(s);
        }
        if let Some(pk) = &src_pk
            && key_field.is_some()
        {
            used_s.insert(pk.clone());
        }
        let fields: Vec<FieldMapping> = chosen
            .iter()
            .map(|s| FieldMapping {
                target: s.target_field.clone(),
                expr: s.expr.clone(),
                confidence: s.confidence,
                status: if s.confidence >= opts.accept_threshold { ReviewStatus::Accepted } else { ReviewStatus::NeedsReview },
                explanation: s.explanation.clone(),
                evidence: s.evidence.clone(),
                deferred: false,
            })
            .collect();
        let unmatched: Vec<String> = so.fields.iter().filter(|f| !used_s.contains(&f.name)).map(|f| f.name.clone()).collect();
        if !unmatched.is_empty() {
            unmatched_source_fields.insert(so.name.clone(), unmatched);
        }
        let missing_req: Vec<String> =
            to.fields.iter().filter(|f| f.required && !f.read_only && !used_t.contains(&f.name)).map(|f| f.name.clone()).collect();
        if !missing_req.is_empty() {
            unmatched_required_targets.insert(to.name.clone(), missing_req);
        }
        objects.push(ObjectMapping {
            source_object: so.name.clone(),
            target_object: to.name.clone(),
            target_key_field: key_field.clone(),
            fields,
            unmapped_fields: UnmappedFieldPolicy::Archive,
            skip_fields: vec![],
            write_mode: if key_field.is_some() { WriteMode::Upsert } else { WriteMode::Insert },
        });
        suggestions.extend(chosen);
    }
    let needs_review = objects.iter().flat_map(|o| &o.fields).filter(|f| f.status == ReviewStatus::NeedsReview).count();
    SuggestionSet {
        pairings,
        suggestions,
        mapping: MappingSpec { mapping_version: 1, objects },
        unmatched_source_fields,
        unmatched_required_targets,
        needs_review,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(name: &str, fields: &[(&str, FieldType)], pk: &str) -> ObjectDef {
        ObjectDef {
            name: name.into(),
            fields: fields.iter().map(|(n, t)| FieldDef::new(*n, *t)).collect(),
            primary_key: vec![pk.into()],
            unique_keys: vec![],
            estimated_count: None,
            readable: true,
            writable: true,
        }
    }

    #[test]
    fn tokenizes() {
        assert_eq!(tokens("billingAddress_line1"), vec!["billing", "address", "line", "1"]);
        assert_eq!(norm("Customers"), "customer");
    }

    #[test]
    fn prompt_example_customer_to_account() {
        let src = SystemSchema {
            objects: vec![obj(
                "Customer",
                &[
                    ("customer_number", FieldType::String),
                    ("legal_name", FieldType::String),
                    ("billing_address", FieldType::String),
                    ("currency", FieldType::String),
                ],
                "customer_number",
            )],
            relationships: vec![],
        };
        let dst = SystemSchema {
            objects: vec![obj(
                "Account",
                &[
                    ("external_id", FieldType::String),
                    ("company_name", FieldType::String),
                    ("address", FieldType::String),
                    ("currency_code", FieldType::String),
                ],
                "external_id",
            )],
            relationships: vec![],
        };
        let set = suggest_mapping(&src, &dst, &[], &SuggestOptions::default());
        assert_eq!(set.pairings.len(), 1);
        let m = &set.mapping.objects[0];
        let target_of = |t: &str| {
            m.fields.iter().find(|f| f.target == t).map(|f| {
                let mut p = vec![];
                f.expr.source_paths(&mut p);
                p.join(",")
            })
        };
        assert_eq!(target_of("external_id").as_deref(), Some("customer_number"));
        assert_eq!(target_of("company_name").as_deref(), Some("legal_name"));
        assert_eq!(target_of("address").as_deref(), Some("billing_address"));
        assert_eq!(target_of("currency_code").as_deref(), Some("currency"));
        assert_eq!(m.target_key_field.as_deref(), Some("external_id"));
        assert!(m.fields.iter().all(|f| !f.explanation.is_empty()));
    }
}
