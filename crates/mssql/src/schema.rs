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
    fk.constraint_name
FROM INFORMATION_SCHEMA.COLUMNS c
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
LEFT JOIN (
    SELECT
        kc.column_name,
        cc.table_schema,
        cc.table_name,
        ccu.table_schema AS foreign_schema,
        ccu.table_name AS foreign_table,
        ccu.column_name AS foreign_column,
        cc.constraint_name
    FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kc
    JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc
        ON rc.constraint_name = kc.constraint_name
    JOIN INFORMATION_SCHEMA.CONSTRAINT_COLUMN_USAGE cc
        ON cc.constraint_name = rc.constraint_name
    JOIN INFORMATION_SCHEMA.CONSTRAINT_COLUMN_USAGE ccu
        ON ccu.constraint_name = rc.unique_constraint_name
) fk ON fk.table_schema = c.TABLE_SCHEMA
    AND fk.table_name = c.TABLE_NAME
    AND fk.column_name = c.COLUMN_NAME
WHERE c.TABLE_NAME = '{table}'
{schema_filter}
ORDER BY c.ORDINAL_POSITION
"#,
        table = table_name.replace('\'', "''"),
        schema_filter = schema_filter,
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
    STRING_AGG(c.name, ',') WITHIN GROUP (ORDER BY ic.key_ordinal)
FROM sys.indexes i
JOIN sys.tables t ON t.object_id = i.object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id
JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
WHERE t.name = '{table}'
{schema_filter}
GROUP BY i.name, i.type_desc, i.is_unique
ORDER BY i.name
"#,
        table = table_name.replace('\'', "''"),
        schema_filter = schema_filter,
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
