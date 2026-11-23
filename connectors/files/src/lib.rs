//! File connector: a directory of `<object>.csv`, `<object>.json` (array) or `<object>.jsonl`.
//!
//! Settings:
//! ```json
//! {"path": "./data", "format": "csv|json|jsonl", "delimiter": ",", "encoding": "utf-8",
//!  "id_fields": {"customers": "customer_number"}, "empty_as_null": true, "overwrite": false}
//! ```
//! Reads stream with byte-offset cursors. Writes are appended and made durable per batch; the
//! durable token is the file length, and on resume the file is truncated back to it, so a crash
//! never leaves duplicates or partial rows.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ma_connector_sdk::{Connector, ConnectorHandle, RecordSink, RecordSource, SinkObjectSpec, WriteRequest, WriteResult, inference};
use ma_types::*;
use serde::Deserialize;
use serde_json::json;

const SCHEMA_SAMPLE: usize = 1000;

/// Rows with their starting byte offsets, plus the offset to resume from (`None` = end).
type RawPage = (Vec<(u64, BTreeMap<String, Value>)>, Option<u64>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Csv,
    Json,
    Jsonl,
}

impl FileFormat {
    fn ext(self) -> &'static str {
        match self {
            FileFormat::Csv => "csv",
            FileFormat::Json => "json",
            FileFormat::Jsonl => "jsonl",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileSettings {
    pub path: PathBuf,
    pub format: FileFormat,
    #[serde(default = "default_delim")]
    pub delimiter: String,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub id_fields: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub empty_as_null: bool,
    /// Allow replacing existing non-empty destination files (destructive; requires
    /// separate authorization in the approval).
    #[serde(default)]
    pub overwrite: bool,
}

fn default_delim() -> String {
    ",".into()
}
fn default_encoding() -> String {
    "utf-8".into()
}
fn default_true() -> bool {
    true
}

struct WriterState {
    out: BufWriter<File>,
    fields: Vec<String>,
    key_field: Option<String>,
    count: u64,
}

pub struct FileConnector {
    settings: FileSettings,
    encoding: &'static encoding_rs::Encoding,
    schemas: Mutex<HashMap<String, ObjectDef>>,
    writers: Mutex<HashMap<String, WriterState>>,
}

impl FileConnector {
    pub fn new(settings: &serde_json::Value) -> MaResult<Self> {
        let s: FileSettings =
            serde_json::from_value(settings.clone()).map_err(|e| MaError::config(format!("invalid file connector settings: {e}")))?;
        if s.delimiter.len() != 1 {
            return Err(MaError::config("delimiter must be a single byte"));
        }
        let encoding = encoding_rs::Encoding::for_label(s.encoding.as_bytes())
            .ok_or_else(|| MaError::config(format!("unknown encoding `{}`", s.encoding)))?;
        if !encoding.is_ascii_compatible() {
            return Err(MaError::unsupported(format!(
                "encoding `{}` is not ASCII-compatible; convert the file to UTF-8 first",
                s.encoding
            )));
        }
        Ok(FileConnector { settings: s, encoding, schemas: Mutex::new(HashMap::new()), writers: Mutex::new(HashMap::new()) })
    }

    pub fn into_handle(self) -> ConnectorHandle {
        let a = Arc::new(self);
        ConnectorHandle { connector: a.clone(), source: Some(a.clone()), sink: Some(a), lookup: None }
    }

    fn file_for(&self, object: &str) -> MaResult<PathBuf> {
        if object.is_empty() || object.contains(['/', '\\', ':']) || object.contains("..") {
            return Err(MaError::validation(format!("invalid object name `{object}`")));
        }
        Ok(self.settings.path.join(format!("{object}.{}", self.settings.format.ext())))
    }

    fn list_objects(&self) -> MaResult<Vec<String>> {
        let mut out = vec![];
        if !self.settings.path.exists() {
            return Ok(out);
        }
        for e in std::fs::read_dir(&self.settings.path)? {
            let p = e?.path();
            if p.extension().and_then(|x| x.to_str()) == Some(self.settings.format.ext())
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                out.push(stem.to_string());
            }
        }
        out.sort();
        Ok(out)
    }

    fn id_field_for(&self, object: &str, headers: &[String]) -> Option<String> {
        if let Some(f) = self.settings.id_fields.get(object) {
            return Some(f.clone());
        }
        headers.iter().find(|h| h.as_str() == "id").cloned()
    }

    fn decode(&self, bytes: &[u8]) -> Value {
        if self.encoding == encoding_rs::UTF_8 {
            match std::str::from_utf8(bytes) {
                Ok(s) => Value::String(s.to_string()),
                Err(e) => Value::Invalid { raw: String::from_utf8_lossy(bytes).into_owned(), reason: format!("invalid UTF-8: {e}") },
            }
        } else {
            let (s, _, had_errors) = self.encoding.decode(bytes);
            if had_errors {
                Value::Invalid { raw: s.into_owned(), reason: format!("invalid {}", self.encoding.name()) }
            } else {
                Value::String(s.into_owned())
            }
        }
    }

    // ------------------------------------------------------------------ raw readers (blocking)

    /// Read raw (untyped) records; CSV cells are `Value::String`.
    fn read_raw(&self, object: &str, offset: u64, limit: usize) -> MaResult<RawPage> {
        let path = self.file_for(object)?;
        let file = File::open(&path).map_err(|e| MaError::not_found(format!("{}: {e}", path.display())))?;
        match self.settings.format {
            FileFormat::Csv => self.read_csv(file, offset, limit),
            FileFormat::Jsonl => read_jsonl(file, offset, limit),
            FileFormat::Json => read_json_array(file, offset, limit),
        }
    }

    fn read_csv(&self, file: File, offset: u64, limit: usize) -> MaResult<RawPage> {
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(self.settings.delimiter.as_bytes()[0])
            .has_headers(true)
            .flexible(true)
            .from_reader(BufReader::with_capacity(256 * 1024, file));
        let headers: Vec<String> = rdr
            .byte_headers()
            .map_err(|e| MaError::validation(format!("CSV header: {e}")))?
            .iter()
            .map(|h| match self.decode(h) {
                Value::String(s) => s.trim_start_matches('\u{feff}').to_string(),
                other => other.to_text().unwrap_or_default(),
            })
            .collect();
        if offset > 0 {
            let mut pos = csv::Position::new();
            pos.set_byte(offset);
            rdr.seek(pos).map_err(|e| MaError::validation(format!("CSV seek: {e}")))?;
        }
        let mut out = Vec::with_capacity(limit.min(10_000));
        let mut rec = csv::ByteRecord::new();
        let mut exhausted = false;
        while out.len() < limit {
            let start = rdr.position().byte();
            match rdr.read_byte_record(&mut rec) {
                Ok(true) => {}
                Ok(false) => {
                    exhausted = true;
                    break;
                }
                Err(e) => return Err(MaError::validation(format!("CSV parse error near byte {start}: {e}"))),
            }
            let mut fields = BTreeMap::new();
            for (i, h) in headers.iter().enumerate() {
                let v = match rec.get(i) {
                    None => continue, // short row: field missing
                    Some(b) if b.is_empty() && self.settings.empty_as_null => Value::Null,
                    Some(b) => self.decode(b),
                };
                fields.insert(h.clone(), v);
            }
            if rec.len() > headers.len() {
                let extra: Vec<String> =
                    (headers.len()..rec.len()).filter_map(|i| rec.get(i)).map(|b| String::from_utf8_lossy(b).into_owned()).collect();
                fields.insert(
                    "_extra_columns".into(),
                    Value::Invalid { raw: extra.join(","), reason: "row has more cells than the header".into() },
                );
            }
            out.push((start, fields));
        }
        let next = if exhausted { None } else { Some(rdr.position().byte()) };
        Ok((out, next))
    }

    fn infer_schema(&self, object: &str) -> MaResult<ObjectDef> {
        if let Some(d) = self.schemas.lock().expect("lock").get(object) {
            return Ok(d.clone());
        }
        let (rows, _) = self.read_raw(object, 0, SCHEMA_SAMPLE)?;
        let typed: Vec<Record> = rows
            .iter()
            .map(|(pos, f)| {
                let mut r = Record::new(object, pos.to_string());
                r.fields = f
                    .iter()
                    .map(|(k, v)| {
                        let v = match (self.settings.format, v) {
                            (FileFormat::Csv, Value::String(s)) => Value::infer_from_text(s),
                            _ => v.clone(),
                        };
                        (k.clone(), v)
                    })
                    .collect();
                r
            })
            .collect();
        let mut headers: Vec<String> = vec![];
        for (_, f) in &rows {
            for k in f.keys() {
                if !headers.contains(k) {
                    headers.push(k.clone());
                }
            }
        }
        let id = self.id_field_for(object, &headers);
        let mut def = inference::infer_object(object, &typed, id.as_deref());
        // Empty CSV cells are nulls (when empty_as_null) so required-ness stays honest.
        def.writable = true;
        self.schemas.lock().expect("lock").insert(object.to_string(), def.clone());
        Ok(def)
    }

    fn typed_record(&self, object: &str, def: &ObjectDef, pos: u64, raw: BTreeMap<String, Value>) -> Record {
        let mut fields = BTreeMap::new();
        for (k, v) in raw {
            let v = match (self.settings.format, v) {
                (FileFormat::Csv, Value::String(s)) => {
                    // The column type was inferred from the first rows only: a later cell that the
                    // type would alter (`01067`, `+46...`, a date with a time) is kept as written.
                    let ty = def.field(&k).map(|f| f.field_type).unwrap_or(FieldType::String);
                    Value::parse_lossless(&s, ty)
                }
                (_, v) => v,
            };
            fields.insert(k, v);
        }
        let id = def.primary_key.first().and_then(|pk| fields.get(pk)).and_then(|v| v.to_text()).unwrap_or_else(|| format!("offset:{pos}"));
        let mut r = Record::new(object, id);
        r.provenance.system = self.settings.path.display().to_string();
        r.provenance.extracted_at = Some(chrono::Utc::now());
        for (k, slot) in [("created_at", 0), ("updated_at", 1)] {
            if let Some(v) = fields.get(k).and_then(|v| v.to_text()) {
                if slot == 0 {
                    r.provenance.source_created_at = Some(v);
                } else {
                    r.provenance.source_updated_at = Some(v);
                }
            }
        }
        r.fields = fields;
        r
    }
}

fn read_jsonl(file: File, offset: u64, limit: usize) -> MaResult<RawPage> {
    let mut rdr = BufReader::with_capacity(256 * 1024, file);
    rdr.seek(SeekFrom::Start(offset))?;
    let mut pos = offset;
    let mut out = Vec::new();
    let mut line = String::new();
    loop {
        if out.len() >= limit {
            return Ok((out, Some(pos)));
        }
        line.clear();
        let n = rdr.read_line(&mut line)?;
        if n == 0 {
            return Ok((out, None));
        }
        let start = pos;
        pos += n as u64;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        out.push((start, json_object(t.as_bytes(), start)?));
    }
}

fn json_object(bytes: &[u8], pos: u64) -> MaResult<BTreeMap<String, Value>> {
    let v = ma_types::json_text::parse(bytes).map_err(|e| MaError::validation(format!("record at byte {pos}: {}", e.message)))?;
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(MaError::validation(format!("JSON value at byte {pos} is not an object"))),
    }
}

/// Stream elements out of a top-level JSON array without loading the file.
fn read_json_array(file: File, offset: u64, limit: usize) -> MaResult<RawPage> {
    let mut rdr = BufReader::with_capacity(256 * 1024, file);
    rdr.seek(SeekFrom::Start(offset))?;
    let mut pos = offset;
    let mut byte = [0u8; 1];
    let mut next = |rdr: &mut BufReader<File>, pos: &mut u64| -> MaResult<Option<u8>> {
        match rdr.read(&mut byte)? {
            0 => Ok(None),
            _ => {
                *pos += 1;
                Ok(Some(byte[0]))
            }
        }
    };
    if offset == 0 {
        loop {
            match next(&mut rdr, &mut pos)? {
                Some(b) if b.is_ascii_whitespace() => continue,
                Some(0xEF) | Some(0xBB) | Some(0xBF) => continue, // BOM
                Some(b'[') => break,
                Some(_) => return Err(MaError::validation("JSON file must contain a top-level array")),
                None => return Ok((vec![], None)),
            }
        }
    }
    let mut out = Vec::new();
    loop {
        if out.len() >= limit {
            return Ok((out, Some(pos)));
        }
        // skip separators
        let first = loop {
            match next(&mut rdr, &mut pos)? {
                Some(b) if b.is_ascii_whitespace() || b == b',' => continue,
                Some(b']') | None => return Ok((out, None)),
                Some(b) => break b,
            }
        };
        let start = pos - 1;
        let mut buf = vec![first];
        let (mut depth, mut in_str, mut esc) = (0i32, false, false);
        match first {
            b'{' | b'[' => depth = 1,
            b'"' => in_str = true,
            _ => return Err(MaError::validation(format!("array element at byte {start} is not an object"))),
        }
        while depth > 0 || in_str {
            let b = next(&mut rdr, &mut pos)?.ok_or_else(|| MaError::validation("unexpected end of JSON"))?;
            buf.push(b);
            if in_str {
                if esc {
                    esc = false;
                } else if b == b'\\' {
                    esc = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                _ => {}
            }
        }
        out.push((start, json_object(&buf, start)?));
    }
}

#[async_trait]
impl Connector for FileConnector {
    fn identify(&self) -> SystemIdentity {
        SystemIdentity {
            kind: "files".into(),
            name: self.settings.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "files".into()),
            version: None,
            locator: format!("{}#{}", self.settings.path.display(), self.settings.format.ext()),
        }
    }

    fn capabilities(&self) -> CapabilityManifest {
        let mut m = CapabilityManifest::new(self.identify());
        m.set("read", Evidence::Verified, Some("streaming, byte-offset cursors".into()))
            .set("write", Evidence::Verified, Some("append with durable truncation-based resume".into()))
            .set("schema_discovery", Evidence::Inferred, Some(format!("types inferred from up to {SCHEMA_SAMPLE} rows")))
            .set("pagination", Evidence::Verified, None)
            .set("upsert", Evidence::Unsupported, Some("files are written once per job; resume is idempotent".into()))
            .set("update", Evidence::Unsupported, None)
            .set("transactions", Evidence::Unsupported, None)
            .set("lookup_by_key", Evidence::Unsupported, None)
            .set("attachments", Evidence::Unsupported, None);
        if self.settings.format == FileFormat::Csv {
            m.limitations.push("CSV cannot distinguish null from empty string or represent nested values natively (nested values are written as JSON text)".into());
        }
        m
    }

    async fn check_connection(&self) -> MaResult<()> {
        let p = &self.settings.path;
        // a destination directory may not exist yet: it is created on first write
        let parent_ok = p.parent().map(|d| d.as_os_str().is_empty() || d.is_dir()).unwrap_or(false);
        if p.is_dir() || (!p.exists() && parent_ok) {
            Ok(())
        } else {
            Err(MaError::config(format!("directory {} does not exist", p.display())))
        }
    }

    async fn discover_schema(&self) -> MaResult<SystemSchema> {
        let mut schema = SystemSchema::default();
        for o in self.list_objects()? {
            let def = self.infer_schema(&o)?;
            schema.objects.push(def);
        }
        // Relationship hints: `<x>_id` fields matching another object's primary key name or name.
        let names: Vec<(String, Vec<String>)> = schema.objects.iter().map(|o| (o.name.clone(), o.primary_key.clone())).collect();
        let mut rels = vec![];
        for o in &schema.objects {
            for f in &o.fields {
                if o.primary_key.contains(&f.name) {
                    continue;
                }
                for (other, pk) in &names {
                    let singular = other.trim_end_matches('s');
                    let pk_name = pk.first().cloned().unwrap_or_else(|| "id".into());
                    if f.name == format!("{singular}_id") || (f.name == pk_name && other != &o.name && pk_name != "id") {
                        rels.push(Relationship {
                            from_object: o.name.clone(),
                            from_field: f.name.clone(),
                            to_object: other.clone(),
                            to_field: pk_name.clone(),
                            evidence: Evidence::Inferred,
                        });
                    }
                }
            }
        }
        schema.relationships = rels;
        Ok(schema)
    }

    fn connector_version(&self) -> String {
        concat!("builtin-", env!("CARGO_PKG_VERSION")).to_string()
    }

    fn creates_objects_on_write(&self) -> bool {
        true
    }
}

#[async_trait]
impl RecordSource for FileConnector {
    async fn read_batch(&self, object: &str, cursor: Option<&Cursor>, limit: usize) -> MaResult<Batch> {
        let offset = cursor.and_then(|c| c.get("offset")).and_then(|v| v.as_u64()).unwrap_or(0);
        let def = self.infer_schema(object)?;
        let (rows, next) = tokio::task::block_in_place(|| self.read_raw(object, offset, limit))?;
        let records = rows.into_iter().map(|(pos, raw)| self.typed_record(object, &def, pos, raw)).collect();
        Ok(Batch { records, next_cursor: next.map(|o| json!({ "offset": o })) })
    }

    async fn count(&self, object: &str) -> MaResult<Option<u64>> {
        let mut n = 0u64;
        let mut offset = 0;
        loop {
            let (rows, next) = tokio::task::block_in_place(|| self.read_raw(object, offset, 50_000))?;
            n += rows.len() as u64;
            match next {
                Some(o) => offset = o,
                None => return Ok(Some(n)),
            }
        }
    }
}

fn csv_cell(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => v.to_text().unwrap_or_default(),
    }
}

#[async_trait]
impl RecordSink for FileConnector {
    async fn prepare(&self, spec: &SinkObjectSpec, resume_token: Option<&Cursor>) -> MaResult<()> {
        std::fs::create_dir_all(&self.settings.path)?;
        let path = self.file_for(&spec.object)?;
        let fields: Vec<String> = spec.fields.iter().map(|f| f.name.clone()).collect();
        let (file, count) = match resume_token {
            Some(tok) => {
                let len = tok.get("len").and_then(|v| v.as_u64()).ok_or_else(|| MaError::internal("bad file token"))?;
                let count = tok.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                let f = OpenOptions::new().write(true).open(&path)?;
                f.set_len(len)?;
                (f, count)
            }
            None => {
                if path.exists() && std::fs::metadata(&path)?.len() > 0 && !self.settings.overwrite {
                    return Err(MaError::policy(format!(
                        "destination file {} already exists and is not empty; remove it or set `overwrite: true` (destructive)",
                        path.display()
                    )));
                }
                let mut f = File::create(&path)?;
                match self.settings.format {
                    FileFormat::Csv => {
                        let mut w = csv::WriterBuilder::new().delimiter(self.settings.delimiter.as_bytes()[0]).from_writer(vec![]);
                        w.write_record(&fields).map_err(|e| MaError::internal(e.to_string()))?;
                        f.write_all(&w.into_inner().map_err(|e| MaError::internal(e.to_string()))?)?;
                    }
                    FileFormat::Json => f.write_all(b"[\n")?,
                    FileFormat::Jsonl => {}
                }
                f.sync_data()?;
                (f, 0)
            }
        };
        let mut file = file;
        file.seek(SeekFrom::End(0))?;
        self.writers.lock().expect("lock").insert(
            spec.object.clone(),
            WriterState { out: BufWriter::with_capacity(256 * 1024, file), fields, key_field: spec.key_field.clone(), count },
        );
        Ok(())
    }

    async fn write_batch(&self, object: &str, records: Vec<WriteRequest>) -> MaResult<Vec<WriteResult>> {
        let delim = self.settings.delimiter.as_bytes()[0];
        let format = self.settings.format;
        let mut writers = self.writers.lock().expect("lock");
        let w = writers.get_mut(object).ok_or_else(|| MaError::internal(format!("sink not prepared for {object}")))?;
        let mut results = Vec::with_capacity(records.len());
        let mut csvw = csv::WriterBuilder::new().delimiter(delim).has_headers(false).from_writer(Vec::with_capacity(64 * 1024));
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        for r in &records {
            let dest_id =
                w.key_field.as_ref().and_then(|k| r.fields.get(k)).and_then(|v| v.to_text()).unwrap_or_else(|| r.source_id.clone());
            match format {
                FileFormat::Csv => {
                    let row: Vec<String> = w.fields.iter().map(|f| csv_cell(r.fields.get(f))).collect();
                    csvw.write_record(&row).map_err(|e| MaError::internal(e.to_string()))?;
                }
                FileFormat::Jsonl | FileFormat::Json => {
                    if format == FileFormat::Json && w.count > 0 {
                        buf.extend_from_slice(b",\n");
                    }
                    ma_types::json_text::write_object(w.fields.iter().filter_map(|f| r.fields.get_key_value(f)), &mut buf);
                    if format == FileFormat::Jsonl {
                        buf.push(b'\n');
                    }
                }
            }
            w.count += 1;
            results.push(Ok(dest_id));
        }
        if format == FileFormat::Csv {
            buf = csvw.into_inner().map_err(|e| MaError::internal(e.to_string()))?;
        }
        w.out.write_all(&buf)?;
        Ok(results)
    }

    async fn durable_token(&self, object: &str) -> MaResult<Option<Cursor>> {
        let mut writers = self.writers.lock().expect("lock");
        let w = writers.get_mut(object).ok_or_else(|| MaError::internal("sink not prepared"))?;
        w.out.flush()?;
        w.out.get_ref().sync_data()?;
        let len = w.out.get_mut().stream_position()?;
        Ok(Some(json!({ "len": len, "count": w.count })))
    }

    async fn finish(&self, object: &str) -> MaResult<()> {
        let mut writers = self.writers.lock().expect("lock");
        if let Some(mut w) = writers.remove(object) {
            if self.settings.format == FileFormat::Json {
                w.out.write_all(b"\n]\n")?;
            }
            w.out.flush()?;
            w.out.get_ref().sync_all()?;
        }
        self.schemas.lock().expect("lock").remove(object);
        Ok(())
    }

    fn idempotent_rewrites(&self, _mode: WriteMode, _has_key: bool) -> bool {
        // Resume truncates back to the durable token, so rewriting a batch cannot duplicate.
        true
    }
}

/// Utility used by tests and benchmarks.
pub fn path_of(dir: &Path, object: &str, format: &str) -> PathBuf {
    dir.join(format!("{object}.{format}"))
}

#[cfg(test)]
mod tests;
