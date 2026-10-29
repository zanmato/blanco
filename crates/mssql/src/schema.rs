/// SQL query builders for SQL Server schema introspection.
///
/// All queries target `INFORMATION_SCHEMA` and `sys` catalog views.
pub fn columns_for_table_sql(table_name: &str, schema: Option<&str>) -> String {
    let schema_filter = match schema {
        Some(s) => format!("AND c.TABLE_SCHEMA = '{}'", s.replace('\'', "''")),
        None => String::new(),
    };
    format!(
        r#"
SELECT
    c.COLUMN_NAME,
    c.DATA_TYPE,
    c.IS_NULLABLE,
    CASE WHEN pk.column_id IS NOT NULL THEN '1' ELSE '0' END,
    c.COLUMN_DEFAULT,
    c.CHARACTER_MAXIMUM_LENGTH,
    fk.foreign_table,
    fk.foreign_column,
    fk.constraint_name,
    CAST(ep.value AS NVARCHAR(4000)),
    c.NUMERIC_PRECISION,
    c.NUMERIC_SCALE
FROM INFORMATION_SCHEMA.COLUMNS c
LEFT JOIN sys.extended_properties ep
    ON ep.class = 1
    AND ep.name = 'MS_Description'
    AND ep.major_id = OBJECT_ID(QUOTENAME(c.TABLE_SCHEMA) + '.' + QUOTENAME(c.TABLE_NAME))
    AND ep.minor_id = COLUMNPROPERTY(ep.major_id, c.COLUMN_NAME, 'ColumnId')
LEFT JOIN (
    -- KEY_COLUMN_USAGE has no `column_id`; `ordinal_position` is the valid
    -- column and serves equally as the non-null PK-membership marker below.
    SELECT kc.table_schema, kc.table_name, kc.column_name, kc.ordinal_position AS column_id
    FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kc
    JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
        ON tc.constraint_name = kc.constraint_name
        AND tc.constraint_schema = kc.constraint_schema
    WHERE tc.constraint_type = 'PRIMARY KEY'
) pk ON pk.table_schema = c.TABLE_SCHEMA
    AND pk.table_name = c.TABLE_NAME
    AND pk.column_name = c.COLUMN_NAME
-- One row per column: a column in several foreign keys keeps one, a single
-- column key ahead of a composite one, so the column is not listed (and keyed
-- on) more than once. `sys.foreign_key_columns` pairs each column with the one
-- it references, which the INFORMATION_SCHEMA views cannot.
LEFT JOIN (
    SELECT
        keyed.column_name,
        keyed.table_schema,
        keyed.table_name,
        keyed.foreign_table,
        keyed.foreign_column,
        keyed.constraint_name,
        ROW_NUMBER() OVER (
            PARTITION BY keyed.table_schema, keyed.table_name, keyed.column_name
            ORDER BY keyed.key_count, keyed.constraint_name
        ) AS key_rank
    FROM (
        SELECT
            pc.name AS column_name,
            SCHEMA_NAME(pt.schema_id) AS table_schema,
            pt.name AS table_name,
            rt.name AS foreign_table,
            rc.name AS foreign_column,
            fk.name AS constraint_name,
            COUNT(*) OVER (PARTITION BY fkc.constraint_object_id) AS key_count
        FROM sys.foreign_keys fk
        JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
        JOIN sys.tables pt ON pt.object_id = fkc.parent_object_id
        JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
        JOIN sys.tables rt ON rt.object_id = fkc.referenced_object_id
        JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
    ) keyed
) fk ON fk.table_schema = c.TABLE_SCHEMA
    AND fk.table_name = c.TABLE_NAME
    AND fk.column_name = c.COLUMN_NAME
    AND fk.key_rank = 1
WHERE c.TABLE_NAME = '{table}'
{schema_filter}
ORDER BY c.ORDINAL_POSITION
"#,
        table = table_name.replace('\'', "''"),
        schema_filter = schema_filter,
    )
}

/// `(kind, object name, comment)` for the tables, views and routines
/// carrying an `MS_Description`, in `schema` or in every schema. `kind` is
/// `r` for tables and views and `f` for routines.
pub fn object_comments_sql(schema: Option<&str>) -> String {
    let schema_filter = match schema {
        Some(s) => format!("AND s.name = '{}'", s.replace('\'', "''")),
        None => String::new(),
    };
    format!(
        r#"
SELECT
    CASE WHEN o.type IN ('U', 'V') THEN 'r' ELSE 'f' END,
    o.name,
    CAST(ep.value AS NVARCHAR(4000))
FROM sys.extended_properties ep
JOIN sys.objects o ON o.object_id = ep.major_id
JOIN sys.schemas s ON s.schema_id = o.schema_id
WHERE ep.class = 1
    AND ep.minor_id = 0
    AND ep.name = 'MS_Description'
    AND o.type IN ('U', 'V', 'P', 'FN', 'IF', 'TF')
    {schema_filter}
"#
    )
}

/// `s.name = N'schema'`, or the session's default schema when none is given.
fn schema_condition(column: &str, schema: Option<&str>) -> String {
    match schema.filter(|schema| !schema.is_empty()) {
        Some(schema) => format!("{column} = N'{}'", schema.replace('\'', "''")),
        None => format!("{column} = SCHEMA_NAME()"),
    }
}

/// One row describing `table_name`: comment, row count, data and index
/// bytes, and creation time.
pub fn table_info_sql(table_name: &str, schema: Option<&str>) -> String {
    format!(
        r#"
SELECT
    CAST(ep.value AS nvarchar(4000)),
    CAST((SELECT SUM(p.rows) FROM sys.partitions p
          WHERE p.object_id = t.object_id AND p.index_id IN (0, 1)) AS varchar(30)),
    CAST((SELECT SUM(a.used_pages) * 8192 FROM sys.partitions p
          JOIN sys.allocation_units a ON a.container_id = p.partition_id
          WHERE p.object_id = t.object_id AND p.index_id IN (0, 1)) AS varchar(30)),
    CAST((SELECT SUM(a.used_pages) * 8192 FROM sys.partitions p
          JOIN sys.allocation_units a ON a.container_id = p.partition_id
          WHERE p.object_id = t.object_id AND p.index_id > 1) AS varchar(30)),
    CONVERT(varchar(19), t.create_date, 120)
FROM sys.tables t
JOIN sys.schemas s ON s.schema_id = t.schema_id
LEFT JOIN sys.extended_properties ep
    ON ep.class = 1 AND ep.major_id = t.object_id AND ep.minor_id = 0
    AND ep.name = 'MS_Description'
WHERE t.name = N'{table}' AND {schema_condition}
"#,
        table = table_name.replace('\'', "''"),
        schema_condition = schema_condition("s.name", schema),
    )
}

/// The last time a statement wrote to `table_name` since the server
/// started, from the index usage statistics.
pub fn table_last_update_sql(table_name: &str, schema: Option<&str>) -> String {
    format!(
        r#"
SELECT CONVERT(varchar(19), MAX(u.last_user_update), 120)
FROM sys.dm_db_index_usage_stats u
JOIN sys.tables t ON t.object_id = u.object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
WHERE u.database_id = DB_ID() AND t.name = N'{table}' AND {schema_condition}
"#,
        table = table_name.replace('\'', "''"),
        schema_condition = schema_condition("s.name", schema),
    )
}

pub fn indexes_for_table_sql(table_name: &str, schema: Option<&str>) -> String {
    let schema_filter = match schema {
        Some(s) => format!("AND s.name = '{}'", s.replace('\'', "''")),
        None => String::new(),
    };
    format!(
        r#"
SELECT
    i.name,
    i.type_desc,
    CASE WHEN i.is_unique = 1 THEN '1' ELSE '0' END,
    STRING_AGG(c.name, ',') WITHIN GROUP (ORDER BY ic.key_ordinal),
    CAST(i.filter_definition AS nvarchar(4000))
FROM sys.indexes i
JOIN sys.tables t ON t.object_id = i.object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id
JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
WHERE t.name = '{table}'
{schema_filter}
GROUP BY i.name, i.type_desc, i.is_unique, CAST(i.filter_definition AS nvarchar(4000))
ORDER BY i.name
"#,
        table = table_name.replace('\'', "''"),
        schema_filter = schema_filter,
    )
}

/// One row per foreign key column of `table_name`, in the shape
/// `ForeignKeyConstraint::from_column_rows` reads.
pub fn foreign_keys_for_table_sql(table_name: &str, schema: Option<&str>) -> String {
    let schema_filter = match schema {
        Some(s) => format!("AND SCHEMA_NAME(t.schema_id) = '{}'", s.replace('\'', "''")),
        None => String::new(),
    };
    format!(
        r#"
SELECT
    fk.name,
    SCHEMA_NAME(rt.schema_id),
    rt.name,
    pc.name,
    rc.name,
    REPLACE(fk.update_referential_action_desc, '_', ' '),
    REPLACE(fk.delete_referential_action_desc, '_', ' ')
FROM sys.foreign_keys fk
JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
JOIN sys.tables t ON t.object_id = fk.parent_object_id
JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id
JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
WHERE t.name = '{table}'
{schema_filter}
ORDER BY fk.name, fkc.constraint_column_id
"#,
        table = table_name.replace('\'', "''"),
        schema_filter = schema_filter,
    )
}

/// Every foreign key column of the tables `get_tables(schema)` lists, one
/// row per listed table and key column. The listed table's name and schema
/// lead, followed by the columns `foreign_keys_for_table_sql` gives. Without
/// a schema a table name matches its namesakes in every schema, as it does
/// table by table.
pub fn schema_foreign_keys_sql(schema: Option<&str>) -> String {
    let (listing_filter, table_filter) = match schema {
        Some(schema) => {
            let schema = schema.replace('\'', "''");
            (
                format!("AND lt.TABLE_SCHEMA = N'{schema}'"),
                format!("AND SCHEMA_NAME(t.schema_id) = N'{schema}'"),
            )
        }
        None => (String::new(), String::new()),
    };
    format!(
        r#"
SELECT
    lt.TABLE_NAME,
    lt.TABLE_SCHEMA,
    fk.name,
    SCHEMA_NAME(rt.schema_id),
    rt.name,
    pc.name,
    rc.name,
    REPLACE(fk.update_referential_action_desc, '_', ' '),
    REPLACE(fk.delete_referential_action_desc, '_', ' ')
FROM INFORMATION_SCHEMA.TABLES lt
JOIN sys.tables t ON t.name = lt.TABLE_NAME {table_filter}
JOIN sys.foreign_keys fk ON fk.parent_object_id = t.object_id
JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id
JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
WHERE lt.TABLE_TYPE = 'BASE TABLE'
{listing_filter}
ORDER BY lt.TABLE_NAME, lt.TABLE_SCHEMA, fk.name, fkc.constraint_column_id
"#
    )
}

/// The columns of the tables `get_tables(schema)` lists (part 0, numbered
/// in name order) and of the `referenced` `(schema, table)` pairs (part 1,
/// numbered in the order given): `(part, position, table, column, data
/// type, primary key flag, table comment, column comment)`. A listed table
/// always gives a row, with NULL columns when none are visible, so it is
/// kept as the per table read keeps it. Types, primary keys and comments are
/// read as `columns_for_table_sql` and `object_comments_sql` read them.
pub fn schema_columns_sql(schema: Option<&str>, referenced: &[(String, String)]) -> String {
    let (own_schema, listing_filter) = match schema {
        Some(schema) => (
            "TABLE_SCHEMA".to_string(),
            format!("AND TABLE_SCHEMA = N'{}'", schema.replace('\'', "''")),
        ),
        None => ("CAST(NULL AS nvarchar(128))".to_string(), String::new()),
    };
    let referenced_rows = if referenced.is_empty() {
        String::new()
    } else {
        let values: Vec<String> = referenced
            .iter()
            .enumerate()
            .map(|(index, (table_schema, table))| {
                format!(
                    "({}, N'{}', N'{}')",
                    index + 1,
                    table_schema.replace('\'', "''"),
                    table.replace('\'', "''"),
                )
            })
            .collect();
        format!(
            r#"
    UNION ALL
    SELECT 1, position, table_name, table_schema,
        OBJECT_ID(QUOTENAME(table_schema) + '.' + QUOTENAME(table_name))
    FROM (VALUES {}) AS referenced(position, table_schema, table_name)"#,
            values.join(", ")
        )
    };
    format!(
        r#"
WITH wanted AS (
    SELECT
        0 AS part,
        ROW_NUMBER() OVER (ORDER BY TABLE_NAME) AS position,
        TABLE_NAME AS table_name,
        {own_schema} AS table_schema,
        OBJECT_ID(QUOTENAME(TABLE_SCHEMA) + '.' + QUOTENAME(TABLE_NAME)) AS object_id
    FROM INFORMATION_SCHEMA.TABLES
    WHERE TABLE_TYPE = 'BASE TABLE'
    {listing_filter}{referenced_rows}
)
SELECT
    w.part,
    w.position,
    w.table_name,
    c.COLUMN_NAME,
    c.DATA_TYPE,
    CASE WHEN pk.column_name IS NOT NULL THEN '1' ELSE '0' END,
    CAST(tep.value AS NVARCHAR(4000)),
    CAST(cep.value AS NVARCHAR(4000))
FROM wanted w
LEFT JOIN sys.extended_properties tep
    ON tep.class = 1
    AND tep.name = 'MS_Description'
    AND tep.major_id = w.object_id
    AND tep.minor_id = 0
LEFT JOIN INFORMATION_SCHEMA.COLUMNS c
    ON c.TABLE_NAME = w.table_name
    AND (w.table_schema IS NULL OR c.TABLE_SCHEMA = w.table_schema)
LEFT JOIN sys.extended_properties cep
    ON cep.class = 1
    AND cep.name = 'MS_Description'
    AND cep.major_id = OBJECT_ID(QUOTENAME(c.TABLE_SCHEMA) + '.' + QUOTENAME(c.TABLE_NAME))
    AND cep.minor_id = COLUMNPROPERTY(cep.major_id, c.COLUMN_NAME, 'ColumnId')
LEFT JOIN (
    SELECT kc.table_schema, kc.table_name, kc.column_name
    FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kc
    JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
        ON tc.constraint_name = kc.constraint_name
        AND tc.constraint_schema = kc.constraint_schema
    WHERE tc.constraint_type = 'PRIMARY KEY'
) pk ON pk.table_schema = c.TABLE_SCHEMA
    AND pk.table_name = c.TABLE_NAME
    AND pk.column_name = c.COLUMN_NAME
ORDER BY w.part, w.position, c.ORDINAL_POSITION, c.TABLE_SCHEMA
"#
    )
}

pub fn database_schema_paginated_sql(table_names: Option<&str>, limit: i64, offset: i64) -> String {
    let name_filter = match table_names {
        Some(names) => {
            let quoted: Vec<String> = names
                .split(',')
                .map(|n| format!("'{}'", n.trim().replace('\'', "''")))
                .collect();
            format!("AND t.name IN ({})", quoted.join(","))
        }
        None => String::new(),
    };

    format!(
        r#"
SELECT t.name, s.name AS schema_name,
    CASE WHEN t.type = 'U' THEN 'TABLE' WHEN t.type = 'V' THEN 'VIEW' ELSE 'UNKNOWN' END AS object_type
FROM sys.objects t
JOIN sys.schemas s ON s.schema_id = t.schema_id
WHERE t.type IN ('U', 'V')
{name_filter}
ORDER BY t.name
OFFSET {offset} ROWS FETCH NEXT {fetch} ROWS ONLY
"#,
        name_filter = name_filter,
        offset = offset,
        fetch = limit + 1,
    )
}
