//! Catalog discovery: tables, columns, keys and foreign keys of one schema.

use std::collections::BTreeMap;
use std::sync::Arc;

use ma_types::{Evidence, FieldDef, FieldType, MaResult, ObjectDef, Relationship, SystemSchema};
use sqlx::{PgPool, Row};

use crate::sql::{pg_err, quote_ident};

/// Column metadata as read from `pg_catalog`.
#[derive(Debug, Clone)]
pub(crate) struct ColMeta {
    pub name: String,
    /// `pg_type.typname` (e.g. `int8`, `_text`).
    pub typname: String,
    /// Element type name for array columns.
    pub elem_typname: Option<String>,
    /// Type used in explicit casts: `format_type(atttypid, NULL)` (no modifier, so explicit
    /// casts never silently truncate or round; assignment rules of the column still apply).
    pub cast_type: String,
    /// Display type with modifier, e.g. `numeric(12,2)`, `int8`, `varchar(50)[]`.
    pub native: String,
    pub field_type: FieldType,
    pub not_null: bool,
    pub has_default: bool,
    /// `a` (ALWAYS), `d` (BY DEFAULT) or empty.
    pub identity: String,
    pub generated: bool,
    pub enum_values: Vec<String>,
    pub description: Option<String>,
}

impl ColMeta {
    pub fn is_array(&self) -> bool {
        self.elem_typname.is_some()
    }
    pub fn is_json(&self) -> bool {
        matches!(self.typname.as_str(), "json" | "jsonb")
    }
    pub fn is_bytea(&self) -> bool {
        self.typname == "bytea"
    }
    pub fn identity_always(&self) -> bool {
        self.identity == "a"
    }
    pub fn read_only(&self) -> bool {
        self.identity_always() || self.generated
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TableMeta {
    pub name: String,
    pub columns: Vec<ColMeta>,
    pub primary_key: Vec<String>,
    pub unique_keys: Vec<Vec<String>>,
}

impl TableMeta {
    pub fn col(&self, name: &str) -> Option<&ColMeta> {
        self.columns.iter().find(|c| c.name == name)
    }
    /// The primary key column when the key has exactly one column.
    pub fn single_pk(&self) -> Option<&ColMeta> {
        match self.primary_key.as_slice() {
            [one] => self.col(one),
            _ => None,
        }
    }
    /// Whether `col` alone is covered by the primary key or a unique constraint/index.
    pub fn is_unique_column(&self, col: &str) -> bool {
        (self.primary_key.len() == 1 && self.primary_key[0] == col) || self.unique_keys.iter().any(|k| k.len() == 1 && k[0] == col)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Catalog {
    pub tables: BTreeMap<String, Arc<TableMeta>>,
    pub relationships: Vec<Relationship>,
}

pub(crate) fn field_type_for(typname: &str, is_array: bool) -> FieldType {
    if is_array {
        return FieldType::Array;
    }
    match typname {
        "int2" | "int4" | "int8" => FieldType::Int,
        "numeric" => FieldType::Decimal,
        "float4" | "float8" => FieldType::Float,
        "bool" => FieldType::Bool,
        "date" => FieldType::Date,
        "timestamp" | "timestamptz" => FieldType::DateTime,
        "json" | "jsonb" => FieldType::Object,
        "bytea" => FieldType::Bytes,
        _ => FieldType::String,
    }
}

/// `character varying(50)[]` -> `(50)`.
fn type_modifier(formatted: &str) -> &str {
    match (formatted.find('('), formatted.find(')')) {
        (Some(a), Some(b)) if b > a => &formatted[a..=b],
        _ => "",
    }
}

const COLUMNS_SQL: &str = r#"
SELECT c.relname::text AS table_name,
       a.attname::text AS column_name,
       t.typname::text AS typname,
       et.typname::text AS elem_typname,
       format_type(a.atttypid, a.atttypmod) AS formatted,
       format_type(a.atttypid, NULL) AS cast_type,
       a.attnotnull AS not_null,
       a.atthasdef AS has_default,
       a.attidentity::text AS identity,
       a.attgenerated::text AS generated,
       CASE WHEN t.typtype = 'e' THEN
         ARRAY(SELECT e.enumlabel::text FROM pg_enum e WHERE e.enumtypid = t.oid ORDER BY e.enumsortorder)
       END AS enum_values,
       col_description(c.oid, a.attnum) AS description
FROM pg_attribute a
JOIN pg_class c ON c.oid = a.attrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
JOIN pg_type t ON t.oid = a.atttypid
LEFT JOIN pg_type et ON t.typcategory = 'A' AND et.oid = t.typelem
WHERE n.nspname = $1
  AND c.relkind IN ('r', 'p')
  AND NOT c.relispartition
  AND a.attnum > 0
  AND NOT a.attisdropped
ORDER BY c.relname, a.attnum
"#;

const CONSTRAINTS_SQL: &str = r#"
SELECT c.relname::text AS table_name,
       con.contype::text AS contype,
       ARRAY(SELECT a.attname::text
             FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord)
             JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
             ORDER BY k.ord) AS cols,
       fc.relname::text AS ref_table,
       fn.nspname::text AS ref_schema,
       ARRAY(SELECT a.attname::text
             FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord)
             JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum
             ORDER BY k.ord) AS ref_cols
FROM pg_constraint con
JOIN pg_class c ON c.oid = con.conrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_class fc ON fc.oid = con.confrelid
LEFT JOIN pg_namespace fn ON fn.oid = fc.relnamespace
WHERE n.nspname = $1
  AND c.relkind IN ('r', 'p')
  AND con.contype IN ('p', 'u', 'f')
ORDER BY c.relname, con.conname
"#;

const UNIQUE_INDEXES_SQL: &str = r#"
SELECT c.relname::text AS table_name,
       ARRAY(SELECT a.attname::text
             FROM unnest(i.indkey::int2[]) WITH ORDINALITY k(attnum, ord)
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
             ORDER BY k.ord) AS cols
FROM pg_index i
JOIN pg_class c ON c.oid = i.indrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
JOIN pg_class ic ON ic.oid = i.indexrelid
WHERE n.nspname = $1
  AND c.relkind IN ('r', 'p')
  AND i.indisunique
  AND NOT i.indisprimary
  AND i.indpred IS NULL
  AND i.indexprs IS NULL
ORDER BY c.relname, ic.relname
"#;

pub(crate) async fn load_catalog(pool: &PgPool, schema: &str) -> MaResult<Catalog> {
    let mut tables: BTreeMap<String, TableMeta> = BTreeMap::new();

    let rows = sqlx::query(COLUMNS_SQL).bind(schema).fetch_all(pool).await.map_err(|e| pg_err("discovering columns", e))?;
    for row in rows {
        let get = |i: &str| -> MaResult<Option<String>> { row.try_get::<Option<String>, _>(i).map_err(|e| pg_err("decoding catalog", e)) };
        let table: String = get("table_name")?.unwrap_or_default();
        let typname = get("typname")?.unwrap_or_default();
        let elem_typname = get("elem_typname")?;
        let formatted = get("formatted")?.unwrap_or_default();
        let base = elem_typname.clone().unwrap_or_else(|| typname.clone());
        let native = format!("{base}{}{}", type_modifier(&formatted), if elem_typname.is_some() { "[]" } else { "" });
        let col = ColMeta {
            name: get("column_name")?.unwrap_or_default(),
            field_type: field_type_for(&typname, elem_typname.is_some()),
            typname,
            elem_typname,
            cast_type: unbounded_cast(&get("cast_type")?.unwrap_or_default()),
            native,
            not_null: row.try_get("not_null").map_err(|e| pg_err("decoding catalog", e))?,
            has_default: row.try_get("has_default").map_err(|e| pg_err("decoding catalog", e))?,
            identity: get("identity")?.unwrap_or_default(),
            generated: !get("generated")?.unwrap_or_default().is_empty(),
            enum_values: row
                .try_get::<Option<Vec<String>>, _>("enum_values")
                .map_err(|e| pg_err("decoding catalog", e))?
                .unwrap_or_default(),
            description: get("description")?,
        };
        tables
            .entry(table.clone())
            .or_insert_with(|| TableMeta { name: table, columns: vec![], primary_key: vec![], unique_keys: vec![] })
            .columns
            .push(col);
    }

    let mut relationships = Vec::new();
    let rows = sqlx::query(CONSTRAINTS_SQL).bind(schema).fetch_all(pool).await.map_err(|e| pg_err("discovering constraints", e))?;
    for row in rows {
        let dec = |e| pg_err("decoding catalog", e);
        let table: String = row.try_get("table_name").map_err(dec)?;
        let contype: String = row.try_get("contype").map_err(dec)?;
        let cols: Vec<String> = row.try_get("cols").map_err(dec)?;
        let Some(t) = tables.get_mut(&table) else { continue };
        match contype.as_str() {
            "p" => t.primary_key = cols,
            "u" => {
                if !t.unique_keys.contains(&cols) {
                    t.unique_keys.push(cols)
                }
            }
            "f" => {
                let ref_table: Option<String> = row.try_get("ref_table").map_err(dec)?;
                let ref_schema: Option<String> = row.try_get("ref_schema").map_err(dec)?;
                let ref_cols: Vec<String> = row.try_get("ref_cols").map_err(dec)?;
                // Only relationships inside the connected schema are representable by name.
                if let (Some(rt), Some(rs)) = (ref_table, ref_schema)
                    && rs == schema
                {
                    for (from, to) in cols.iter().zip(ref_cols.iter()) {
                        relationships.push(Relationship {
                            from_object: table.clone(),
                            from_field: from.clone(),
                            to_object: rt.clone(),
                            to_field: to.clone(),
                            evidence: Evidence::Documented,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    let rows = sqlx::query(UNIQUE_INDEXES_SQL).bind(schema).fetch_all(pool).await.map_err(|e| pg_err("discovering unique indexes", e))?;
    for row in rows {
        let dec = |e| pg_err("decoding catalog", e);
        let table: String = row.try_get("table_name").map_err(dec)?;
        let cols: Vec<String> = row.try_get("cols").map_err(dec)?;
        if let Some(t) = tables.get_mut(&table)
            && !cols.is_empty()
            && cols != t.primary_key
            && !t.unique_keys.contains(&cols)
        {
            t.unique_keys.push(cols);
        }
    }

    Ok(Catalog { tables: tables.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(), relationships })
}

pub(crate) async fn exact_count(pool: &PgPool, schema: &str, table: &str) -> MaResult<u64> {
    let sql = format!("SELECT count(*) FROM {}.{}", quote_ident(schema), quote_ident(table));
    let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql)).fetch_one(pool).await.map_err(|e| pg_err("counting rows", e))?;
    Ok(n.max(0) as u64)
}

pub(crate) fn object_def(t: &TableMeta, count: Option<u64>) -> ObjectDef {
    ObjectDef {
        name: t.name.clone(),
        fields: t
            .columns
            .iter()
            .map(|c| FieldDef {
                name: c.name.clone(),
                field_type: c.field_type,
                nullable: !c.not_null,
                required: c.not_null && !c.has_default && c.identity.is_empty() && !c.generated,
                custom: false,
                description: c.description.clone(),
                enum_values: c.enum_values.clone(),
                read_only: c.read_only(),
                native_type: Some(c.native.clone()),
            })
            .collect(),
        primary_key: t.primary_key.clone(),
        unique_keys: t.unique_keys.clone(),
        estimated_count: count,
        readable: true,
        writable: true,
    }
}

pub(crate) async fn system_schema(pool: &PgPool, schema: &str, catalog: &Catalog) -> MaResult<SystemSchema> {
    let mut objects = Vec::with_capacity(catalog.tables.len());
    for t in catalog.tables.values() {
        let count = exact_count(pool, schema, &t.name).await?;
        objects.push(object_def(t, Some(count)));
    }
    Ok(SystemSchema { objects, relationships: catalog.relationships.clone() })
}

/// `format_type(oid, NULL)` yields `character` / `bit`, which PostgreSQL interprets as length 1
/// in a cast — that would silently truncate `char(n)` values. Use the unbounded type names so the
/// column's own type modifier (or an error) applies on assignment.
fn unbounded_cast(t: &str) -> String {
    match t {
        "character" => "bpchar".into(),
        "character[]" => "bpchar[]".into(),
        "bit" => "varbit".into(),
        "bit[]" => "varbit[]".into(),
        other => other.to_string(),
    }
}
