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
    pub description: TypeDescription,
}

/// What a type holds, in the interface language when it is displayed. The
/// tables are statics, built before the app chooses its language, so they
/// hold the key under `blanco_core.data_type` rather than the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeDescription(&'static str);

impl std::fmt::Display for TypeDescription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!(format!("blanco_core.data_type.{}", self.0)))
    }
}

const fn plain(name: &'static str, description: &'static str) -> DataTypeName {
    DataTypeName {
        name,
        arguments: None,
        description: TypeDescription(description),
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
        description: TypeDescription(description),
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
    plain("smallint", "n2_byte_integer"),
    plain("integer", "n4_byte_integer"),
    plain("bigint", "n8_byte_integer"),
    sized("numeric", "p, s", "exact_number"),
    plain("real", "n4_byte_float"),
    plain("double precision", "n8_byte_float"),
    plain("smallserial", "auto_increment_smallint"),
    plain("serial", "auto_increment_integer"),
    plain("bigserial", "auto_increment_bigint"),
    plain("money", "currency_amount"),
    plain("text", "unbounded_text"),
    sized("varchar", "n", "text_up_to_n_characters"),
    sized("char", "n", "fixed_length_text"),
    plain("bytea", "binary_data"),
    plain("boolean", "true_or_false"),
    plain("date", "calendar_date"),
    plain("time", "time_of_day"),
    plain("timetz", "time_of_day_with_time_zone"),
    plain("timestamp", "date_and_time"),
    plain("timestamptz", "date_and_time_with_time_zone"),
    plain("interval", "time_span"),
    plain("uuid", "universally_unique_identifier"),
    plain("json", "json_text"),
    plain("jsonb", "binary_json"),
    plain("xml", "xml_document"),
    plain("inet", "ip_address"),
    plain("cidr", "ip_network"),
    plain("macaddr", "mac_address"),
    plain("macaddr8", "mac_address_eui_64"),
    sized("bit", "n", "fixed_length_bit_string"),
    sized("varbit", "n", "bit_string_up_to_n_bits"),
    plain("point", "geometric_point"),
    plain("line", "infinite_line"),
    plain("lseg", "line_segment"),
    plain("box", "rectangle"),
    plain("path", "geometric_path"),
    plain("polygon", "closed_path"),
    plain("circle", "circle"),
    plain("tsvector", "text_search_document"),
    plain("tsquery", "text_search_query"),
    plain("int4range", "range_of_integer"),
    plain("int8range", "range_of_bigint"),
    plain("numrange", "range_of_numeric"),
    plain("tsrange", "range_of_timestamp"),
    plain("tstzrange", "range_of_timestamptz"),
    plain("daterange", "range_of_date"),
    plain("oid", "object_identifier"),
];

const COCKROACH: &[DataTypeName] = &[
    plain("int2", "n2_byte_integer"),
    plain("int4", "n4_byte_integer"),
    plain("int8", "n8_byte_integer"),
    sized("decimal", "p, s", "exact_number"),
    plain("float4", "n4_byte_float"),
    plain("float8", "n8_byte_float"),
    plain("serial", "unique_row_id"),
    plain("string", "unbounded_text"),
    sized("varchar", "n", "text_up_to_n_characters"),
    sized("char", "n", "fixed_length_text"),
    plain("bytes", "binary_data"),
    plain("bool", "true_or_false"),
    plain("date", "calendar_date"),
    plain("time", "time_of_day"),
    plain("timetz", "time_of_day_with_time_zone"),
    plain("timestamp", "date_and_time"),
    plain("timestamptz", "date_and_time_with_time_zone"),
    plain("interval", "time_span"),
    plain("uuid", "universally_unique_identifier"),
    plain("jsonb", "binary_json"),
    plain("inet", "ip_address"),
    sized("bit", "n", "fixed_length_bit_string"),
    sized("varbit", "n", "bit_string_up_to_n_bits"),
    plain("geometry", "spatial_shape"),
    plain("geography", "spatial_shape_on_the_globe"),
    plain("oid", "object_identifier"),
];

const MYSQL: &[DataTypeName] = &[
    plain("tinyint", "n1_byte_integer"),
    plain("smallint", "n2_byte_integer"),
    plain("mediumint", "n3_byte_integer"),
    plain("int", "n4_byte_integer"),
    plain("bigint", "n8_byte_integer"),
    sized("decimal", "p, s", "exact_number"),
    plain("float", "n4_byte_float"),
    plain("double", "n8_byte_float"),
    sized("bit", "n", "bit_field"),
    plain("boolean", "tinyint_1"),
    sized("char", "n", "fixed_length_text"),
    sized("varchar", "n", "text_up_to_n_characters"),
    sized("binary", "n", "fixed_length_bytes"),
    sized("varbinary", "n", "bytes_up_to_n"),
    plain("tinytext", "text_up_to_255_bytes"),
    plain("text", "text_up_to_64_kb"),
    plain("mediumtext", "text_up_to_16_mb"),
    plain("longtext", "text_up_to_4_gb"),
    plain("tinyblob", "bytes_up_to_255"),
    plain("blob", "bytes_up_to_64_kb"),
    plain("mediumblob", "bytes_up_to_16_mb"),
    plain("longblob", "bytes_up_to_4_gb"),
    sized("enum", "'a', 'b'", "one_of_a_list"),
    sized("set", "'a', 'b'", "any_of_a_list"),
    plain("date", "calendar_date"),
    plain("time", "time_of_day"),
    plain("datetime", "date_and_time"),
    plain("timestamp", "date_and_time_stored_as_utc"),
    plain("year", "n4_digit_year"),
    plain("json", "json_document"),
    plain("uuid", "uuid_mariadb"),
    plain("inet4", "ipv4_address_mariadb"),
    plain("inet6", "ipv6_address_mariadb"),
    plain("geometry", "spatial_shape"),
    plain("point", "spatial_point"),
    plain("linestring", "spatial_line"),
    plain("polygon", "spatial_polygon"),
];

const SQLITE: &[DataTypeName] = &[
    plain("INTEGER", "integer_affinity"),
    plain("REAL", "float_affinity"),
    plain("TEXT", "text_affinity"),
    plain("BLOB", "stored_as_given"),
    plain("NUMERIC", "numeric_affinity"),
    plain("INT", "integer_affinity"),
    plain("BIGINT", "integer_affinity"),
    sized("VARCHAR", "n", "text_affinity_length_not_enforced"),
    plain("BOOLEAN", "numeric_affinity"),
    plain("DATE", "numeric_affinity"),
    plain("DATETIME", "numeric_affinity"),
    sized("DECIMAL", "p, s", "numeric_affinity"),
    plain("DOUBLE", "float_affinity"),
    plain("FLOAT", "float_affinity"),
];

const MSSQL: &[DataTypeName] = &[
    plain("bit", "n0_or_1"),
    plain("tinyint", "n1_byte_unsigned_integer"),
    plain("smallint", "n2_byte_integer"),
    plain("int", "n4_byte_integer"),
    plain("bigint", "n8_byte_integer"),
    sized("decimal", "p, s", "exact_number"),
    sized("numeric", "p, s", "exact_number"),
    plain("money", "currency_amount"),
    plain("smallmoney", "small_currency_amount"),
    plain("float", "n8_byte_float"),
    plain("real", "n4_byte_float"),
    plain("date", "calendar_date"),
    plain("time", "time_of_day"),
    plain("datetime", "date_and_time"),
    plain("datetime2", "date_and_time_finer_precision"),
    plain("datetimeoffset", "date_and_time_with_offset"),
    plain("smalldatetime", "date_and_time_to_the_minute"),
    sized("char", "n", "fixed_length_text"),
    sized("varchar", "n", "text_up_to_n_characters"),
    plain("varchar(max)", "text_up_to_2_gb"),
    sized("nchar", "n", "fixed_length_unicode_text"),
    sized("nvarchar", "n", "unicode_text_up_to_n_characters"),
    plain("nvarchar(max)", "unicode_text_up_to_2_gb"),
    sized("binary", "n", "fixed_length_bytes"),
    sized("varbinary", "n", "bytes_up_to_n"),
    plain("varbinary(max)", "bytes_up_to_2_gb"),
    plain("uniqueidentifier", "guid"),
    plain("xml", "xml_document"),
    plain("rowversion", "row_change_counter"),
    plain("sql_variant", "value_of_any_type"),
    plain("hierarchyid", "position_in_a_hierarchy"),
    plain("geography", "spatial_shape_on_the_globe"),
    plain("geometry", "spatial_shape"),
];

const CLICKHOUSE: &[DataTypeName] = &[
    sized("Nullable", "T", "t_or_null"),
    sized("LowCardinality", "T", "dictionary_encoded_t"),
    plain("UInt8", "n1_byte_unsigned_integer"),
    plain("UInt16", "n2_byte_unsigned_integer"),
    plain("UInt32", "n4_byte_unsigned_integer"),
    plain("UInt64", "n8_byte_unsigned_integer"),
    plain("UInt128", "n16_byte_unsigned_integer"),
    plain("UInt256", "n32_byte_unsigned_integer"),
    plain("Int8", "n1_byte_integer"),
    plain("Int16", "n2_byte_integer"),
    plain("Int32", "n4_byte_integer"),
    plain("Int64", "n8_byte_integer"),
    plain("Int128", "n16_byte_integer"),
    plain("Int256", "n32_byte_integer"),
    plain("Float32", "n4_byte_float"),
    plain("Float64", "n8_byte_float"),
    sized("Decimal", "P, S", "exact_number"),
    plain("Bool", "true_or_false"),
    plain("String", "bytes_of_any_length"),
    sized("FixedString", "N", "exactly_n_bytes"),
    plain("UUID", "universally_unique_identifier"),
    plain("Date", "calendar_date"),
    plain("Date32", "calendar_date_wider_range"),
    plain("DateTime", "date_and_time_to_the_second"),
    sized("DateTime64", "precision", "date_and_time_with_sub_seconds"),
    sized("Enum8", "'a' = 1", "one_of_up_to_256_values"),
    sized("Enum16", "'a' = 1", "one_of_up_to_65536_values"),
    sized("Array", "T", "list_of_t"),
    sized("Map", "K, V", "keys_of_k_to_values_of_v"),
    sized("Tuple", "T1, T2", "fixed_list_of_values"),
    plain("IPv4", "ipv4_address"),
    plain("IPv6", "ipv6_address"),
    plain("JSON", "json_object"),
];

const DUCKDB: &[DataTypeName] = &[
    plain("BOOLEAN", "true_or_false"),
    plain("TINYINT", "n1_byte_integer"),
    plain("SMALLINT", "n2_byte_integer"),
    plain("INTEGER", "n4_byte_integer"),
    plain("BIGINT", "n8_byte_integer"),
    plain("HUGEINT", "n16_byte_integer"),
    plain("UTINYINT", "n1_byte_unsigned_integer"),
    plain("USMALLINT", "n2_byte_unsigned_integer"),
    plain("UINTEGER", "n4_byte_unsigned_integer"),
    plain("UBIGINT", "n8_byte_unsigned_integer"),
    plain("UHUGEINT", "n16_byte_unsigned_integer"),
    plain("FLOAT", "n4_byte_float"),
    plain("DOUBLE", "n8_byte_float"),
    sized("DECIMAL", "p, s", "exact_number"),
    plain("VARCHAR", "text"),
    plain("BLOB", "binary_data"),
    plain("BIT", "bit_string"),
    plain("DATE", "calendar_date"),
    plain("TIME", "time_of_day"),
    plain("TIMETZ", "time_of_day_with_time_zone"),
    plain("TIMESTAMP", "date_and_time"),
    plain("TIMESTAMPTZ", "date_and_time_with_time_zone"),
    plain("TIMESTAMP_S", "date_and_time_to_the_second"),
    plain("TIMESTAMP_MS", "date_and_time_to_the_millisecond"),
    plain("TIMESTAMP_NS", "date_and_time_to_the_nanosecond"),
    plain("INTERVAL", "time_span"),
    plain("UUID", "universally_unique_identifier"),
    plain("JSON", "json_text"),
    sized("STRUCT", "name type", "named_fields"),
    sized("MAP", "K, V", "keys_of_k_to_values_of_v"),
    sized("UNION", "name type", "one_of_several_types"),
    sized("ENUM", "'a', 'b'", "one_of_a_list"),
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

    #[test]
    fn every_type_description_is_translated() {
        let backend = crate::_rust_i18n_backend();
        for db_type in DatabaseType::ALL {
            for data_type in db_type.dialect().data_types() {
                let key = format!("blanco_core.data_type.{}", data_type.description.0);
                for locale in ["en", "sv"] {
                    assert!(
                        backend.translate(locale, &key).is_some(),
                        "{locale}: missing {key}"
                    );
                }
            }
        }
    }
}
