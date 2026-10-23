//! The built in type names each backend spells, for suggestions in the
//! structure tab's type field. Types that take a length or precision carry
//! the argument's name, so a suggestion can leave the caret between the
//! parentheses.

use super::Dialect;
use crate::DatabaseType;

/// One type name as a backend spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataTypeName {
    pub name: &'static str,
    /// What goes between the parentheses (`n`, `p, s`), when the type takes
    /// arguments.
    pub arguments: Option<&'static str>,
    pub description: &'static str,
}

const fn plain(name: &'static str, description: &'static str) -> DataTypeName {
    DataTypeName {
        name,
        arguments: None,
        description,
    }
}

const fn sized(
    name: &'static str,
    arguments: &'static str,
    description: &'static str,
) -> DataTypeName {
    DataTypeName {
        name,
        arguments: Some(arguments),
        description,
    }
}

impl DataTypeName {
    /// The name with its arguments, as a list shows it: `varchar(n)`.
    pub fn label(&self) -> String {
        match self.arguments {
            Some(arguments) => format!("{}({arguments})", self.name),
            None => self.name.to_string(),
        }
    }
}

const POSTGRES: &[DataTypeName] = &[
    plain("smallint", "2 byte integer"),
    plain("integer", "4 byte integer"),
    plain("bigint", "8 byte integer"),
    sized("numeric", "p, s", "exact number"),
    plain("real", "4 byte float"),
    plain("double precision", "8 byte float"),
    plain("smallserial", "auto increment smallint"),
    plain("serial", "auto increment integer"),
    plain("bigserial", "auto increment bigint"),
    plain("money", "currency amount"),
    plain("text", "unbounded text"),
    sized("varchar", "n", "text up to n characters"),
    sized("char", "n", "fixed length text"),
    plain("bytea", "binary data"),
    plain("boolean", "true or false"),
    plain("date", "calendar date"),
    plain("time", "time of day"),
    plain("timetz", "time of day with time zone"),
    plain("timestamp", "date and time"),
    plain("timestamptz", "date and time with time zone"),
    plain("interval", "time span"),
    plain("uuid", "universally unique identifier"),
    plain("json", "JSON text"),
    plain("jsonb", "binary JSON"),
    plain("xml", "XML document"),
    plain("inet", "IP address"),
    plain("cidr", "IP network"),
    plain("macaddr", "MAC address"),
    plain("macaddr8", "MAC address, EUI-64"),
    sized("bit", "n", "fixed length bit string"),
    sized("varbit", "n", "bit string up to n bits"),
    plain("point", "geometric point"),
    plain("line", "infinite line"),
    plain("lseg", "line segment"),
    plain("box", "rectangle"),
    plain("path", "geometric path"),
    plain("polygon", "closed path"),
    plain("circle", "circle"),
    plain("tsvector", "text search document"),
    plain("tsquery", "text search query"),
    plain("int4range", "range of integer"),
    plain("int8range", "range of bigint"),
    plain("numrange", "range of numeric"),
    plain("tsrange", "range of timestamp"),
    plain("tstzrange", "range of timestamptz"),
    plain("daterange", "range of date"),
    plain("oid", "object identifier"),
];

const COCKROACH: &[DataTypeName] = &[
    plain("int2", "2 byte integer"),
    plain("int4", "4 byte integer"),
    plain("int8", "8 byte integer"),
    sized("decimal", "p, s", "exact number"),
    plain("float4", "4 byte float"),
    plain("float8", "8 byte float"),
    plain("serial", "unique row id"),
    plain("string", "unbounded text"),
    sized("varchar", "n", "text up to n characters"),
    sized("char", "n", "fixed length text"),
    plain("bytes", "binary data"),
    plain("bool", "true or false"),
    plain("date", "calendar date"),
    plain("time", "time of day"),
    plain("timetz", "time of day with time zone"),
    plain("timestamp", "date and time"),
    plain("timestamptz", "date and time with time zone"),
    plain("interval", "time span"),
    plain("uuid", "universally unique identifier"),
    plain("jsonb", "binary JSON"),
    plain("inet", "IP address"),
    sized("bit", "n", "fixed length bit string"),
    sized("varbit", "n", "bit string up to n bits"),
    plain("geometry", "spatial shape"),
    plain("geography", "spatial shape on the globe"),
    plain("oid", "object identifier"),
];

const MYSQL: &[DataTypeName] = &[
    plain("tinyint", "1 byte integer"),
    plain("smallint", "2 byte integer"),
    plain("mediumint", "3 byte integer"),
    plain("int", "4 byte integer"),
    plain("bigint", "8 byte integer"),
    sized("decimal", "p, s", "exact number"),
    plain("float", "4 byte float"),
    plain("double", "8 byte float"),
    sized("bit", "n", "bit field"),
    plain("boolean", "tinyint(1)"),
    sized("char", "n", "fixed length text"),
    sized("varchar", "n", "text up to n characters"),
    sized("binary", "n", "fixed length bytes"),
    sized("varbinary", "n", "bytes up to n"),
    plain("tinytext", "text up to 255 bytes"),
    plain("text", "text up to 64 KB"),
    plain("mediumtext", "text up to 16 MB"),
    plain("longtext", "text up to 4 GB"),
    plain("tinyblob", "bytes up to 255"),
    plain("blob", "bytes up to 64 KB"),
    plain("mediumblob", "bytes up to 16 MB"),
    plain("longblob", "bytes up to 4 GB"),
    sized("enum", "'a', 'b'", "one of a list"),
    sized("set", "'a', 'b'", "any of a list"),
    plain("date", "calendar date"),
    plain("time", "time of day"),
    plain("datetime", "date and time"),
    plain("timestamp", "date and time, stored as UTC"),
    plain("year", "4 digit year"),
    plain("json", "JSON document"),
    plain("uuid", "UUID (MariaDB)"),
    plain("inet4", "IPv4 address (MariaDB)"),
    plain("inet6", "IPv6 address (MariaDB)"),
    plain("geometry", "spatial shape"),
    plain("point", "spatial point"),
    plain("linestring", "spatial line"),
    plain("polygon", "spatial polygon"),
];

const SQLITE: &[DataTypeName] = &[
    plain("INTEGER", "integer affinity"),
    plain("REAL", "float affinity"),
    plain("TEXT", "text affinity"),
    plain("BLOB", "stored as given"),
    plain("NUMERIC", "numeric affinity"),
    plain("INT", "integer affinity"),
    plain("BIGINT", "integer affinity"),
    sized("VARCHAR", "n", "text affinity, length not enforced"),
    plain("BOOLEAN", "numeric affinity"),
    plain("DATE", "numeric affinity"),
    plain("DATETIME", "numeric affinity"),
    sized("DECIMAL", "p, s", "numeric affinity"),
    plain("DOUBLE", "float affinity"),
    plain("FLOAT", "float affinity"),
];

const MSSQL: &[DataTypeName] = &[
    plain("bit", "0 or 1"),
    plain("tinyint", "1 byte unsigned integer"),
    plain("smallint", "2 byte integer"),
    plain("int", "4 byte integer"),
    plain("bigint", "8 byte integer"),
    sized("decimal", "p, s", "exact number"),
    sized("numeric", "p, s", "exact number"),
    plain("money", "currency amount"),
    plain("smallmoney", "small currency amount"),
    plain("float", "8 byte float"),
    plain("real", "4 byte float"),
    plain("date", "calendar date"),
    plain("time", "time of day"),
    plain("datetime", "date and time"),
    plain("datetime2", "date and time, finer precision"),
    plain("datetimeoffset", "date and time with offset"),
    plain("smalldatetime", "date and time to the minute"),
    sized("char", "n", "fixed length text"),
    sized("varchar", "n", "text up to n characters"),
    plain("varchar(max)", "text up to 2 GB"),
    sized("nchar", "n", "fixed length Unicode text"),
    sized("nvarchar", "n", "Unicode text up to n characters"),
    plain("nvarchar(max)", "Unicode text up to 2 GB"),
    sized("binary", "n", "fixed length bytes"),
    sized("varbinary", "n", "bytes up to n"),
    plain("varbinary(max)", "bytes up to 2 GB"),
    plain("uniqueidentifier", "GUID"),
    plain("xml", "XML document"),
    plain("rowversion", "row change counter"),
    plain("sql_variant", "value of any type"),
    plain("hierarchyid", "position in a hierarchy"),
    plain("geography", "spatial shape on the globe"),
    plain("geometry", "spatial shape"),
];

const CLICKHOUSE: &[DataTypeName] = &[
    sized("Nullable", "T", "T or NULL"),
    sized("LowCardinality", "T", "dictionary encoded T"),
    plain("UInt8", "1 byte unsigned integer"),
    plain("UInt16", "2 byte unsigned integer"),
    plain("UInt32", "4 byte unsigned integer"),
    plain("UInt64", "8 byte unsigned integer"),
    plain("UInt128", "16 byte unsigned integer"),
    plain("UInt256", "32 byte unsigned integer"),
    plain("Int8", "1 byte integer"),
    plain("Int16", "2 byte integer"),
    plain("Int32", "4 byte integer"),
    plain("Int64", "8 byte integer"),
    plain("Int128", "16 byte integer"),
    plain("Int256", "32 byte integer"),
    plain("Float32", "4 byte float"),
    plain("Float64", "8 byte float"),
    sized("Decimal", "P, S", "exact number"),
    plain("Bool", "true or false"),
    plain("String", "bytes of any length"),
    sized("FixedString", "N", "exactly N bytes"),
    plain("UUID", "universally unique identifier"),
    plain("Date", "calendar date"),
    plain("Date32", "calendar date, wider range"),
    plain("DateTime", "date and time to the second"),
    sized("DateTime64", "precision", "date and time with sub seconds"),
    sized("Enum8", "'a' = 1", "one of up to 256 values"),
    sized("Enum16", "'a' = 1", "one of up to 65536 values"),
    sized("Array", "T", "list of T"),
    sized("Map", "K, V", "keys of K to values of V"),
    sized("Tuple", "T1, T2", "fixed list of values"),
    plain("IPv4", "IPv4 address"),
    plain("IPv6", "IPv6 address"),
    plain("JSON", "JSON object"),
];

const DUCKDB: &[DataTypeName] = &[
    plain("BOOLEAN", "true or false"),
    plain("TINYINT", "1 byte integer"),
    plain("SMALLINT", "2 byte integer"),
    plain("INTEGER", "4 byte integer"),
    plain("BIGINT", "8 byte integer"),
    plain("HUGEINT", "16 byte integer"),
    plain("UTINYINT", "1 byte unsigned integer"),
    plain("USMALLINT", "2 byte unsigned integer"),
    plain("UINTEGER", "4 byte unsigned integer"),
    plain("UBIGINT", "8 byte unsigned integer"),
    plain("UHUGEINT", "16 byte unsigned integer"),
    plain("FLOAT", "4 byte float"),
    plain("DOUBLE", "8 byte float"),
    sized("DECIMAL", "p, s", "exact number"),
    plain("VARCHAR", "text"),
    plain("BLOB", "binary data"),
    plain("BIT", "bit string"),
    plain("DATE", "calendar date"),
    plain("TIME", "time of day"),
    plain("TIMETZ", "time of day with time zone"),
    plain("TIMESTAMP", "date and time"),
    plain("TIMESTAMPTZ", "date and time with time zone"),
    plain("TIMESTAMP_S", "date and time to the second"),
    plain("TIMESTAMP_MS", "date and time to the millisecond"),
    plain("TIMESTAMP_NS", "date and time to the nanosecond"),
    plain("INTERVAL", "time span"),
    plain("UUID", "universally unique identifier"),
    plain("JSON", "JSON text"),
    sized("STRUCT", "name type", "named fields"),
    sized("MAP", "K, V", "keys of K to values of V"),
    sized("UNION", "name type", "one of several types"),
    sized("ENUM", "'a', 'b'", "one of a list"),
];

impl Dialect {
    /// The backend's built in type names, in the order a suggestion list
    /// shows them before anything is typed.
    pub fn data_types(&self) -> &'static [DataTypeName] {
        match self.0 {
            DatabaseType::PostgreSQL => POSTGRES,
            DatabaseType::CockroachDb => COCKROACH,
            DatabaseType::MySQL => MYSQL,
            DatabaseType::SQLite => SQLITE,
            DatabaseType::MsSql => MSSQL,
            DatabaseType::ClickHouse => CLICKHOUSE,
            DatabaseType::DuckDb => DUCKDB,
            DatabaseType::Redis => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::DatabaseType;

    #[test]
    fn every_sql_backend_lists_unique_types() {
        for db_type in DatabaseType::ALL {
            let types = db_type.dialect().data_types();
            assert_eq!(types.is_empty(), !db_type.dialect().supports_sql());
            let mut names: Vec<String> = types.iter().map(|t| t.label().to_lowercase()).collect();
            names.sort();
            let count = names.len();
            names.dedup();
            assert_eq!(names.len(), count, "{db_type:?} lists a type twice");
        }
    }
}
