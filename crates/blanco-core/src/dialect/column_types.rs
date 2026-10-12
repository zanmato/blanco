//! What a backend's type name means for the grid, which decodes, aligns
//! and colours values by it. The drivers map the types of their results
//! here, and the structure tab its declared column types.

use super::Dialect;
use crate::{ColumnType, DatabaseType};

impl Dialect {
    /// The kind of values a column of `type_name` holds, as the driver names
    /// the type of a result column.
    pub fn column_type(&self, type_name: &str) -> ColumnType {
        match self.database_type() {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => postgres(type_name),
            DatabaseType::MySQL => mysql(type_name),
            DatabaseType::SQLite => sqlite(type_name),
            DatabaseType::MsSql => mssql(type_name),
            DatabaseType::ClickHouse => clickhouse(type_name),
            DatabaseType::DuckDb => duckdb(type_name),
            DatabaseType::Redis => ColumnType::Unknown,
        }
    }

    /// The kind of values a column declared as `declared` holds. Catalogs
    /// write a type with its arguments (`character varying(40)`,
    /// `int(10) unsigned`, `timestamp(6) with time zone`), which result
    /// columns leave out. ClickHouse's arguments are types of their own, so
    /// its names are taken as they are.
    pub fn declared_column_type(&self, declared: &str) -> ColumnType {
        if self.database_type() == DatabaseType::ClickHouse {
            return self.column_type(declared);
        }
        let mut bare = String::with_capacity(declared.len());
        let mut depth = 0usize;
        for character in declared.chars() {
            match character {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => bare.push(character),
                _ => {}
            }
        }
        let bare = bare.split_whitespace().collect::<Vec<_>>().join(" ");
        self.column_type(&bare)
    }
}

/// Map PostgreSQL type name to ColumnType enum.
///
/// The set of types here is the result of comparing what
/// `pg_catalog.pg_type` exposes against what we used to handle. Many
/// PostgreSQL types (network, geometric, ranges, ts*, regX OID aliases,
/// xml, bit strings, jsonpath, pg_lsn, etc.) come back over the wire as
/// readable text. Mapping them to `Text` (rather than `Unknown`) routes
/// them through the string decoder, which is fast and avoids a
/// "unknown column type" warning per row.
fn postgres(type_name: &str) -> ColumnType {
    let lower = type_name.to_lowercase();
    match lower.as_str() {
        // Integers (signed + serial)
        "smallint" | "int2" | "smallserial" | "int" | "int4" | "integer" | "serial" | "bigint"
        | "int8" | "bigserial" => ColumnType::Integer,
        // PostgreSQL object identifier integers
        "oid" | "xid" | "xid8" | "cid" | "tid" => ColumnType::Integer,
        // Numerics
        "real" | "float4" | "double precision" | "float8" | "numeric" | "decimal" | "money" => {
            ColumnType::Numeric
        }
        "boolean" | "bool" => ColumnType::Boolean,
        // Character / text family (note: "char" is the 1-byte type, char is bpchar)
        "text" | "varchar" | "character varying" | "char" | "\"char\"" | "bpchar" | "character"
        | "name" => ColumnType::Text,
        // Date / time
        "timestamp"
        | "timestamp without time zone"
        | "timestamptz"
        | "timestamp with time zone"
        | "date"
        | "time"
        | "time without time zone"
        | "timetz"
        | "time with time zone"
        | "interval" => ColumnType::DateTime,
        "uuid" => ColumnType::Uuid,
        "json" | "jsonb" | "jsonpath" => ColumnType::Json,
        // An extension type without a fixed OID, known by its name.
        "hstore" => ColumnType::KeyValue,
        "bytea" => ColumnType::Binary,
        // PostGIS, and CockroachDB's own spatial types.
        "geometry" | "geography" => ColumnType::Geometry,
        "array" => ColumnType::Array,
        _ if lower.ends_with("[]") => ColumnType::Array,
        // Network address types: come back as text
        "inet" | "cidr" | "macaddr" | "macaddr8" => ColumnType::Text,
        // Bit strings: text representation is "1010..."
        "bit" | "varbit" | "bit varying" => ColumnType::Text,
        // Geometric types: rendered as text like "(1,2),(3,4)"
        "point" | "line" | "lseg" | "box" | "path" | "polygon" | "circle" => ColumnType::Text,
        // Full-text search
        "tsvector" | "tsquery" | "gtsvector" => ColumnType::Text,
        // Ranges and multiranges (built-in)
        "int4range" | "int8range" | "numrange" | "daterange" | "tsrange" | "tstzrange"
        | "int4multirange" | "int8multirange" | "nummultirange" | "datemultirange"
        | "tsmultirange" | "tstzmultirange" => ColumnType::Text,
        // System catalog identifier aliases (regclass, regtype, etc.)
        "regclass" | "regcollation" | "regconfig" | "regdictionary" | "regnamespace"
        | "regoper" | "regoperator" | "regproc" | "regprocedure" | "regrole" | "regtype" => {
            ColumnType::Text
        }
        // Misc text-renderable
        "xml" | "pg_lsn" | "aclitem" | "pg_snapshot" | "txid_snapshot" => ColumnType::Text,
        _ => ColumnType::Unknown,
    }
}

fn mysql(type_name: &str) -> ColumnType {
    let type_lower = type_name.to_lowercase();

    // Check for unsigned integer types first
    if type_lower.ends_with("unsigned") {
        let base_type = type_lower.trim_end_matches("unsigned").trim();
        return match base_type {
            "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => ColumnType::UnsignedInteger,
            _ => ColumnType::Unknown,
        };
    }

    match type_lower.as_str() {
        "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => ColumnType::Integer,
        "float" | "double" | "decimal" | "numeric" => ColumnType::Numeric,
        "char" | "varchar" | "text" => ColumnType::Text,
        "date" | "datetime" | "timestamp" | "time" | "year" => ColumnType::DateTime,
        "json" => ColumnType::Json,
        "binary" | "varbinary" | "blob" | "tinyblob" | "mediumblob" | "longblob" => {
            ColumnType::Binary
        }
        "boolean" => ColumnType::Boolean,
        _ => ColumnType::Unknown,
    }
}

fn sqlite(type_name: &str) -> ColumnType {
    match type_name.to_lowercase().as_str() {
        "integer" | "int" => ColumnType::Integer,
        "real" | "float" | "double" | "numeric" | "decimal" => ColumnType::Numeric,
        "text" | "varchar" => ColumnType::Text,
        "boolean" => ColumnType::Boolean,
        "date" | "datetime" => ColumnType::DateTime,
        "blob" => ColumnType::Binary,
        _ => ColumnType::Unknown,
    }
}

fn mssql(type_name: &str) -> ColumnType {
    let lower = type_name.to_lowercase();
    let bare = lower.split('(').next().unwrap_or(&lower).trim();

    match bare {
        "bit" => ColumnType::Boolean,
        "tinyint" | "smallint" | "int" | "bigint" => ColumnType::Integer,
        "real" | "float" | "decimal" | "numeric" | "money" | "smallmoney" => ColumnType::Numeric,
        "date" | "time" | "datetime" | "datetime2" | "smalldatetime" | "datetimeoffset" => {
            ColumnType::DateTime
        }
        "uniqueidentifier" => ColumnType::Uuid,
        "varbinary" | "binary" | "image" => ColumnType::Binary,
        "text" | "ntext" | "varchar" | "nvarchar" | "char" | "nchar" | "xml" => ColumnType::Text,
        "json" => ColumnType::Json,
        _ => ColumnType::Unknown,
    }
}

fn clickhouse(type_name: &str) -> ColumnType {
    let inner = unwrap_type_wrappers(type_name);
    let lower = inner.to_lowercase();

    if lower.starts_with("uint") {
        ColumnType::UnsignedInteger
    } else if lower.starts_with("int")
        || lower == "bigint"
        || lower == "smallint"
        || lower == "tinyint"
        || lower == "mediumint"
        || lower == "serial"
        || lower == "bigserial"
    {
        ColumnType::Integer
    } else if lower.starts_with("float")
        || lower.starts_with("double")
        || lower == "real"
        || lower.starts_with("decimal")
        || lower.starts_with("numeric")
        || lower == "money"
    {
        ColumnType::Numeric
    } else if lower == "bool" || lower == "boolean" {
        ColumnType::Boolean
    } else if lower.starts_with("date")
        || lower.starts_with("datetime")
        || lower == "time"
        || lower == "timestamp"
    {
        ColumnType::DateTime
    } else if lower == "uuid" {
        ColumnType::Uuid
    } else if lower == "json" || lower.starts_with("object") {
        ColumnType::Json
    } else if lower.starts_with("array") {
        ColumnType::Array
    } else if lower.starts_with("string")
        || lower.starts_with("fixedstring")
        || lower.starts_with("enum")
        || lower.starts_with("lowcardinality")
        || lower == "name"
        || lower == "nullable"
    {
        ColumnType::Text
    } else {
        ColumnType::Unknown
    }
}

fn unwrap_type_wrappers(type_name: &str) -> &str {
    let s = type_name.trim();
    if let Some(inner) = s.strip_prefix("Nullable(")
        && let Some(inner) = inner.strip_suffix(')')
    {
        return unwrap_type_wrappers(inner);
    }
    if let Some(inner) = s.strip_prefix("LowCardinality(")
        && let Some(inner) = inner.strip_suffix(')')
    {
        return unwrap_type_wrappers(inner);
    }
    s
}

/// DuckDB's type names. Its results are typed through Arrow instead, so
/// this serves declared column types.
fn duckdb(type_name: &str) -> ColumnType {
    let lower = type_name.trim().to_lowercase();
    if lower.ends_with("[]") || lower.starts_with("list") {
        return ColumnType::Array;
    }
    match lower.as_str() {
        "utinyint" | "usmallint" | "uinteger" | "ubigint" | "uhugeint" => {
            ColumnType::UnsignedInteger
        }
        "tinyint" | "smallint" | "integer" | "int" | "bigint" | "hugeint" | "int1" | "int2"
        | "int4" | "int8" => ColumnType::Integer,
        "float" | "real" | "double" | "decimal" | "numeric" => ColumnType::Numeric,
        "boolean" | "bool" => ColumnType::Boolean,
        "varchar" | "text" | "string" | "char" | "bpchar" | "bit" => ColumnType::Text,
        "date"
        | "time"
        | "timetz"
        | "timestamp"
        | "timestamptz"
        | "timestamp with time zone"
        | "timestamp_s"
        | "timestamp_ms"
        | "timestamp_ns"
        | "datetime"
        | "interval" => ColumnType::DateTime,
        "uuid" => ColumnType::Uuid,
        "json" => ColumnType::Json,
        "blob" | "bytea" | "varbinary" => ColumnType::Binary,
        _ => ColumnType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use crate::{ColumnType, DatabaseType};

    #[test]
    fn declared_types_lose_their_arguments() {
        let declared =
            |db_type: DatabaseType, name: &str| db_type.dialect().declared_column_type(name);
        assert_eq!(
            declared(DatabaseType::PostgreSQL, "character varying(40)"),
            ColumnType::Text
        );
        assert_eq!(
            declared(DatabaseType::PostgreSQL, "timestamp(6) with time zone"),
            ColumnType::DateTime
        );
        assert_eq!(
            declared(DatabaseType::PostgreSQL, "numeric(12,2)"),
            ColumnType::Numeric
        );
        assert_eq!(
            declared(DatabaseType::PostgreSQL, "integer[]"),
            ColumnType::Array
        );
        assert_eq!(
            declared(DatabaseType::MySQL, "int(10) unsigned"),
            ColumnType::UnsignedInteger
        );
        assert_eq!(
            declared(DatabaseType::ClickHouse, "Nullable(DateTime64(3))"),
            ColumnType::DateTime
        );
        assert_eq!(declared(DatabaseType::DuckDb, "UUID"), ColumnType::Uuid);
        assert_eq!(
            declared(DatabaseType::DuckDb, "INTEGER[]"),
            ColumnType::Array
        );
        assert_eq!(
            declared(DatabaseType::MsSql, "nvarchar(max)"),
            ColumnType::Text
        );
    }
}
