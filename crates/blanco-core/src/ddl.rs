//! Statement builders for the small set of DDL operations offered from the
//! sidebar. Each backend spells these differently, so the dialect knowledge
//! lives here rather than in the UI.

use crate::{DatabaseType, EntityType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableOperation {
    Drop,
    Truncate,
    Rename { new_name: String },
}

impl TableOperation {
    pub fn label(&self) -> &'static str {
        match self {
            TableOperation::Drop => "Drop",
            TableOperation::Truncate => "Truncate",
            TableOperation::Rename { .. } => "Rename",
        }
    }
}

/// Quote one identifier for `db_type`. Input is taken as a bare name; any
/// quote characters in it are escaped, not interpreted.
pub fn quote_identifier(db_type: DatabaseType, identifier: &str) -> String {
    db_type.dialect().quote_identifier(identifier)
}

fn qualified(db_type: DatabaseType, schema: Option<&str>, name: &str) -> String {
    db_type.dialect().quote_qualified(schema, name)
}

/// Build the statement for `operation` on `name`, or `None` when the backend
/// or object kind does not support it (Redis has no tables; views cannot be
/// truncated).
pub fn table_operation_sql(
    db_type: DatabaseType,
    entity: EntityType,
    schema: Option<&str>,
    name: &str,
    operation: &TableOperation,
) -> Option<String> {
    if db_type == DatabaseType::Redis {
        return None;
    }
    let target = qualified(db_type, schema, name);
    let keyword = match entity {
        EntityType::Table => "TABLE",
        EntityType::View => "VIEW",
        EntityType::MaterializedView => "MATERIALIZED VIEW",
    };
    match operation {
        TableOperation::Drop => Some(format!("DROP {keyword} {target}")),
        TableOperation::Truncate => {
            if entity != EntityType::Table {
                return None;
            }
            Some(match db_type {
                // SQLite has no TRUNCATE; an unqualified DELETE is its
                // optimised equivalent.
                DatabaseType::SQLite => format!("DELETE FROM {target}"),
                _ => format!("TRUNCATE TABLE {target}"),
            })
        }
        TableOperation::Rename { new_name } => {
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return None;
            }
            Some(match db_type {
                DatabaseType::MsSql => {
                    let old = match schema.filter(|schema| !schema.is_empty()) {
                        Some(schema) => format!("{schema}.{name}"),
                        None => name.to_string(),
                    };
                    format!(
                        "EXEC sp_rename '{}', '{}'",
                        old.replace('\'', "''"),
                        new_name.replace('\'', "''")
                    )
                }
                DatabaseType::ClickHouse => format!(
                    "RENAME TABLE {target} TO {}",
                    qualified(db_type, schema, new_name)
                ),
                _ => format!(
                    "ALTER {keyword} {target} RENAME TO {}",
                    quote_identifier(db_type, new_name)
                ),
            })
        }
    }
}

/// A column as the user described it in the structure tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColumnSpec {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    /// Raw default expression (`0`, `'n/a'`, `now()`), `None` for no default.
    pub default: Option<String>,
    pub comment: Option<String>,
    /// Clauses MySQL keeps beside the type that restating the column must
    /// repeat (`auto_increment`, `on update current_timestamp()`).
    pub attributes: Option<String>,
    /// A generated column. MySQL cannot restate one without its expression.
    pub generated: bool,
    /// Part of the primary key of a table being created. Changing the key
    /// of an existing table is left to a query.
    pub primary_key: bool,
}

impl ColumnSpec {
    fn trimmed_default(&self) -> Option<&str> {
        self.default
            .as_deref()
            .map(str::trim)
            .filter(|default| !default.is_empty())
    }

    fn trimmed_comment(&self) -> Option<&str> {
        self.comment
            .as_deref()
            .map(str::trim)
            .filter(|comment| !comment.is_empty())
    }
}

/// Where a column goes among the table's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnPosition {
    First,
    After(String),
}

impl ColumnPosition {
    fn sql(&self, db_type: DatabaseType) -> String {
        match self {
            ColumnPosition::First => "FIRST".to_string(),
            ColumnPosition::After(name) => {
                format!("AFTER {}", db_type.dialect().quote_identifier(name.trim()))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnOperation {
    /// Add a column, after the others unless `position` says where.
    Add {
        column: ColumnSpec,
        position: Option<ColumnPosition>,
    },
    /// Move the column `column` describes. MySQL moves a column only by
    /// restating it, so the move carries its whole definition.
    Move {
        column: ColumnSpec,
        position: ColumnPosition,
    },
    Rename {
        from: String,
        to: String,
    },
    Drop {
        name: String,
    },
    /// Change the column described by `from` into `to`. Only what differs
    /// is changed, so a backend that cannot change one property in place
    /// still takes a change to the others.
    Alter {
        from: ColumnSpec,
        to: ColumnSpec,
    },
}

impl ColumnOperation {
    pub fn label(&self) -> &'static str {
        match self {
            ColumnOperation::Add { .. } => "Add column",
            ColumnOperation::Move { .. } => "Move column",
            ColumnOperation::Rename { .. } => "Rename column",
            ColumnOperation::Drop { .. } => "Drop column",
            ColumnOperation::Alter { .. } => "Alter column",
        }
    }
}

/// An index as the user described it in the structure tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexSpec {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    /// A unique index that counts NULLs as equal, so at most one row may
    /// leave its columns NULL (PostgreSQL 15 and later).
    pub nulls_not_distinct: bool,
    /// The access method (`gin`) or, on ClickHouse, the skipping index type.
    /// `None` takes the backend's default.
    pub method: Option<String>,
    /// The `WHERE` condition of a partial index.
    pub condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOperation {
    Create(IndexSpec),
    Drop { name: String },
}

impl IndexOperation {
    pub fn label(&self) -> &'static str {
        match self {
            IndexOperation::Create(_) => "Create index",
            IndexOperation::Drop { .. } => "Drop index",
        }
    }
}

/// A change to the table itself rather than to one of its columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableChange {
    /// Set the table's comment, or remove it with `None`.
    Comment(Option<String>),
}

impl TableChange {
    pub fn label(&self) -> &'static str {
        match self {
            TableChange::Comment(_) => "Change table comment",
        }
    }
}

/// What happens to referencing rows when the referenced row is updated or
/// deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferentialAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl ReferentialAction {
    pub const ALL: [ReferentialAction; 5] = [
        ReferentialAction::NoAction,
        ReferentialAction::Restrict,
        ReferentialAction::Cascade,
        ReferentialAction::SetNull,
        ReferentialAction::SetDefault,
    ];

    /// The clause keyword, also how catalogs report the action.
    pub fn sql(&self) -> &'static str {
        match self {
            ReferentialAction::NoAction => "NO ACTION",
            ReferentialAction::Restrict => "RESTRICT",
            ReferentialAction::Cascade => "CASCADE",
            ReferentialAction::SetNull => "SET NULL",
            ReferentialAction::SetDefault => "SET DEFAULT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignKeyOperation {
    Create {
        /// Constraint name; `None` lets the server pick one.
        name: Option<String>,
        columns: Vec<String>,
        /// Schema of the referenced table; `None` means the table's own.
        referenced_schema: Option<String>,
        referenced_table: String,
        referenced_columns: Vec<String>,
        on_update: ReferentialAction,
        on_delete: ReferentialAction,
    },
    Drop {
        name: String,
    },
}

impl ForeignKeyOperation {
    pub fn label(&self) -> &'static str {
        match self {
            ForeignKeyOperation::Create { .. } => "Create foreign key",
            ForeignKeyOperation::Drop { .. } => "Drop foreign key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdlError {
    /// The backend has no statement for this operation.
    Unsupported(String),
    /// The request itself is malformed (empty name, no columns, ...).
    Invalid(String),
}

impl std::fmt::Display for DdlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DdlError::Unsupported(message) | DdlError::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DdlError {}

fn require_name(name: &str, what: &str) -> Result<(), DdlError> {
    if name.trim().is_empty() {
        Err(DdlError::Invalid(format!("{what} name must not be empty")))
    } else {
        Ok(())
    }
}

fn validate_spec(spec: &ColumnSpec) -> Result<(), DdlError> {
    require_name(&spec.name, "column")?;
    if spec.data_type.trim().is_empty() {
        return Err(DdlError::Invalid(
            "column type must not be empty".to_string(),
        ));
    }
    Ok(())
}

/// `type [NOT NULL] [DEFAULT x]` in the shape each backend accepts in a
/// column definition. ClickHouse expresses nullability in the type.
fn column_definition(db_type: DatabaseType, spec: &ColumnSpec) -> String {
    let dialect = db_type.dialect();
    let mut definition = dialect.quote_identifier(spec.name.trim());
    definition.push(' ');
    let data_type = spec.data_type.trim();
    match db_type {
        DatabaseType::ClickHouse => definition.push_str(&clickhouse_type(spec)),
        DatabaseType::MsSql => {
            definition.push_str(data_type);
            definition.push_str(if spec.nullable { " NULL" } else { " NOT NULL" });
        }
        _ => {
            definition.push_str(data_type);
            if !spec.nullable {
                definition.push_str(" NOT NULL");
            }
        }
    }
    if let Some(default) = spec.trimmed_default() {
        definition.push_str(&format!(" DEFAULT {default}"));
    }
    if db_type == DatabaseType::MySQL
        && let Some(attributes) = spec
            .attributes
            .as_deref()
            .map(str::trim)
            .filter(|attributes| !attributes.is_empty())
    {
        definition.push(' ');
        definition.push_str(attributes);
    }
    // MySQL and ClickHouse keep a column's comment in its definition, and
    // MySQL's CHANGE COLUMN drops a comment the definition leaves out.
    if matches!(db_type, DatabaseType::MySQL | DatabaseType::ClickHouse)
        && let Some(comment) = spec.trimmed_comment()
    {
        definition.push_str(&format!(" COMMENT {}", dialect.string_literal(comment)));
    }
    definition
}

/// ClickHouse spells a nullable column's type `Nullable(T)`.
fn clickhouse_type(spec: &ColumnSpec) -> String {
    let data_type = spec.data_type.trim();
    let already_nullable = data_type.to_ascii_lowercase().starts_with("nullable(");
    if spec.nullable && !already_nullable {
        format!("Nullable({data_type})")
    } else {
        data_type.to_string()
    }
}

fn unsupported_positions(db_type: DatabaseType) -> DdlError {
    DdlError::Unsupported(format!(
        "{} adds columns only after the others and cannot move them",
        db_type.as_str()
    ))
}

fn unsupported_comments(db_type: DatabaseType) -> DdlError {
    DdlError::Unsupported(format!("{} has no comments", db_type.as_str()))
}

/// `N'text'`, T-SQL's Unicode string literal.
fn mssql_literal(value: &str) -> String {
    format!("N'{}'", value.replace('\'', "''"))
}

/// SQL Server keeps comments as `MS_Description` extended properties, which
/// are added, updated and dropped by separate procedures. One `IF` statement
/// picks the procedure from whether the property exists.
fn mssql_description_sql(
    schema: Option<&str>,
    table: &str,
    column: Option<&str>,
    comment: Option<&str>,
) -> String {
    let dialect = DatabaseType::MsSql.dialect();
    let schema = schema.filter(|schema| !schema.is_empty()).unwrap_or("dbo");
    let object = mssql_literal(&dialect.quote_qualified(Some(schema), table));
    let mut levels = format!(
        "@level0type = N'SCHEMA', @level0name = {}, @level1type = N'TABLE', @level1name = {}",
        mssql_literal(schema),
        mssql_literal(table)
    );
    let minor_id = match column {
        Some(column) => {
            levels.push_str(&format!(
                ", @level2type = N'COLUMN', @level2name = {}",
                mssql_literal(column)
            ));
            format!(
                "COLUMNPROPERTY(OBJECT_ID({object}), {}, 'ColumnId')",
                mssql_literal(column)
            )
        }
        None => "0".to_string(),
    };
    let exists = format!(
        "EXISTS (SELECT 1 FROM sys.extended_properties WHERE class = 1 AND name = N'MS_Description' AND major_id = OBJECT_ID({object}) AND minor_id = {minor_id})"
    );
    match comment {
        Some(comment) => {
            let value = mssql_literal(comment);
            format!(
                "IF {exists} EXEC sp_updateextendedproperty @name = N'MS_Description', @value = {value}, {levels} ELSE EXEC sp_addextendedproperty @name = N'MS_Description', @value = {value}, {levels}"
            )
        }
        None => {
            format!("IF {exists} EXEC sp_dropextendedproperty @name = N'MS_Description', {levels}")
        }
    }
}

/// The statement that sets (or with `None` removes) the comment of
/// `column`, for the backends that comment a column apart from its
/// definition. MySQL restates the whole definition instead.
fn column_comment_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    column: &str,
    comment: Option<&str>,
) -> Result<String, DdlError> {
    let dialect = db_type.dialect();
    let column = column.trim();
    match db_type {
        DatabaseType::PostgreSQL | DatabaseType::CockroachDb | DatabaseType::DuckDb => Ok(format!(
            "COMMENT ON COLUMN {}.{} IS {}",
            dialect.quote_qualified(schema, table),
            dialect.quote_identifier(column),
            comment.map_or("NULL".to_string(), |comment| dialect
                .string_literal(comment))
        )),
        DatabaseType::ClickHouse => Ok(format!(
            "ALTER TABLE {} COMMENT COLUMN {} {}",
            dialect.quote_qualified(schema, table),
            dialect.quote_identifier(column),
            dialect.string_literal(comment.unwrap_or_default())
        )),
        DatabaseType::MsSql => Ok(mssql_description_sql(schema, table, Some(column), comment)),
        DatabaseType::MySQL | DatabaseType::SQLite | DatabaseType::Redis => {
            Err(unsupported_comments(db_type))
        }
    }
}

/// Statements for a change to the table itself.
pub fn table_change_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    change: &TableChange,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    let target = dialect.quote_qualified(schema, table);
    match change {
        TableChange::Comment(comment) => {
            let comment = comment
                .as_deref()
                .map(str::trim)
                .filter(|comment| !comment.is_empty());
            Ok(vec![match db_type {
                DatabaseType::PostgreSQL | DatabaseType::CockroachDb | DatabaseType::DuckDb => {
                    format!(
                        "COMMENT ON TABLE {target} IS {}",
                        comment.map_or("NULL".to_string(), |comment| dialect
                            .string_literal(comment))
                    )
                }
                DatabaseType::MySQL => format!(
                    "ALTER TABLE {target} COMMENT = {}",
                    dialect.string_literal(comment.unwrap_or_default())
                ),
                DatabaseType::ClickHouse => format!(
                    "ALTER TABLE {target} MODIFY COMMENT {}",
                    dialect.string_literal(comment.unwrap_or_default())
                ),
                DatabaseType::MsSql => mssql_description_sql(schema, table, None, comment),
                DatabaseType::SQLite | DatabaseType::Redis => {
                    return Err(unsupported_comments(db_type));
                }
            }])
        }
    }
}

/// Statements for `operation` on `table`. More than one statement is returned
/// when the backend needs separate steps (a rename plus a modify); run them
/// through `execute_operations_transactional` so a later failure rolls back
/// the earlier ones where the backend allows transactional DDL.
pub fn column_operation_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    operation: &ColumnOperation,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    if !dialect.supports_column_ddl() {
        return Err(DdlError::Unsupported(format!(
            "{} has no column DDL",
            db_type.as_str()
        )));
    }
    let target = dialect.quote_qualified(schema, table);
    let quoted = |name: &str| dialect.quote_identifier(name.trim());

    match operation {
        ColumnOperation::Add {
            column: spec,
            position,
        } => {
            validate_spec(spec)?;
            let comment = spec.trimmed_comment();
            if comment.is_some() && !dialect.supports_comments() {
                return Err(unsupported_comments(db_type));
            }
            if position.is_some() && !dialect.supports_column_positions() {
                return Err(unsupported_positions(db_type));
            }
            let placement = position
                .as_ref()
                .map(|position| format!(" {}", position.sql(db_type)))
                .unwrap_or_default();
            let mut statements = Vec::new();
            if db_type == DatabaseType::DuckDb && !spec.nullable {
                // DuckDB adds no column with a constraint, so the NOT NULL
                // follows once the column exists (its default fills it).
                let nullable = ColumnSpec {
                    nullable: true,
                    ..spec.clone()
                };
                statements.push(format!(
                    "ALTER TABLE {target} ADD COLUMN {}",
                    column_definition(db_type, &nullable)
                ));
                statements.push(format!(
                    "ALTER TABLE {target} ALTER COLUMN {} SET NOT NULL",
                    quoted(&spec.name)
                ));
            } else {
                let definition = column_definition(db_type, spec);
                statements.push(match db_type {
                    // T-SQL has no COLUMN keyword in ADD.
                    DatabaseType::MsSql => format!("ALTER TABLE {target} ADD {definition}"),
                    _ => format!("ALTER TABLE {target} ADD COLUMN {definition}{placement}"),
                });
            }
            if comment.is_some()
                && !matches!(db_type, DatabaseType::MySQL | DatabaseType::ClickHouse)
            {
                statements.push(column_comment_sql(
                    db_type, schema, table, &spec.name, comment,
                )?);
            }
            Ok(statements)
        }
        ColumnOperation::Move {
            column: spec,
            position,
        } => {
            validate_spec(spec)?;
            if let ColumnPosition::After(name) = position {
                require_name(name, "preceding column")?;
                if name.trim() == spec.name.trim() {
                    return Err(DdlError::Invalid(
                        "a column cannot follow itself".to_string(),
                    ));
                }
            }
            let placement = position.sql(db_type);
            match db_type {
                DatabaseType::MySQL => {
                    if spec.generated {
                        return Err(DdlError::Unsupported(
                            "MySQL moves a generated column only with its expression; move it in a query".to_string(),
                        ));
                    }
                    Ok(vec![format!(
                        "ALTER TABLE {target} MODIFY COLUMN {} {placement}",
                        column_definition(db_type, spec)
                    )])
                }
                // ClickHouse keeps the default, codec, TTL and comment of a
                // column modified with its type alone.
                DatabaseType::ClickHouse => Ok(vec![format!(
                    "ALTER TABLE {target} MODIFY COLUMN {} {} {placement}",
                    quoted(&spec.name),
                    clickhouse_type(spec)
                )]),
                _ => Err(unsupported_positions(db_type)),
            }
        }
        ColumnOperation::Rename { from, to } => {
            require_name(from, "column")?;
            require_name(to, "new column")?;
            Ok(vec![rename_column_sql(db_type, schema, table, from, to)])
        }
        ColumnOperation::Drop { name } => {
            require_name(name, "column")?;
            Ok(vec![format!(
                "ALTER TABLE {target} DROP COLUMN {}",
                quoted(name)
            )])
        }
        ColumnOperation::Alter { from, to } => {
            validate_spec(from)?;
            validate_spec(to)?;
            let renamed = from.name.trim() != to.name.trim();
            let type_changed = from.data_type.trim() != to.data_type.trim();
            let null_changed = from.nullable != to.nullable;
            let default_changed = from.trimmed_default() != to.trimmed_default();
            let comment_changed = from.trimmed_comment() != to.trimmed_comment();
            let definition_changed = type_changed || null_changed || default_changed;
            if !(renamed || definition_changed || comment_changed) {
                return Err(DdlError::Invalid("the column is unchanged".to_string()));
            }
            if comment_changed && !dialect.supports_comments() {
                return Err(unsupported_comments(db_type));
            }
            let column = quoted(&to.name);
            let rename = || rename_column_sql(db_type, schema, table, &from.name, &to.name);
            let comment =
                || column_comment_sql(db_type, schema, table, &to.name, to.trimmed_comment());
            let mut statements = Vec::new();
            match db_type {
                DatabaseType::PostgreSQL | DatabaseType::CockroachDb => {
                    if renamed {
                        statements.push(rename());
                    }
                    let mut actions = Vec::new();
                    if type_changed {
                        actions.push(format!(
                            "ALTER COLUMN {column} TYPE {}",
                            to.data_type.trim()
                        ));
                    }
                    if null_changed {
                        actions.push(format!(
                            "ALTER COLUMN {column} {}",
                            if to.nullable {
                                "DROP NOT NULL"
                            } else {
                                "SET NOT NULL"
                            }
                        ));
                    }
                    if default_changed {
                        actions.push(match to.trimmed_default() {
                            Some(default) => format!("ALTER COLUMN {column} SET DEFAULT {default}"),
                            None => format!("ALTER COLUMN {column} DROP DEFAULT"),
                        });
                    }
                    if !actions.is_empty() {
                        statements.push(format!("ALTER TABLE {target} {}", actions.join(", ")));
                    }
                    if comment_changed {
                        statements.push(comment()?);
                    }
                }
                DatabaseType::DuckDb => {
                    // DuckDB takes one action per ALTER TABLE.
                    if renamed {
                        statements.push(rename());
                    }
                    if type_changed {
                        statements.push(format!(
                            "ALTER TABLE {target} ALTER COLUMN {column} TYPE {}",
                            to.data_type.trim()
                        ));
                    }
                    if null_changed {
                        statements.push(format!(
                            "ALTER TABLE {target} ALTER COLUMN {column} {}",
                            if to.nullable {
                                "DROP NOT NULL"
                            } else {
                                "SET NOT NULL"
                            }
                        ));
                    }
                    if default_changed {
                        statements.push(match to.trimmed_default() {
                            Some(default) => format!(
                                "ALTER TABLE {target} ALTER COLUMN {column} SET DEFAULT {default}"
                            ),
                            None => {
                                format!("ALTER TABLE {target} ALTER COLUMN {column} DROP DEFAULT")
                            }
                        });
                    }
                    if comment_changed {
                        statements.push(comment()?);
                    }
                }
                DatabaseType::MySQL => {
                    if from.generated && (definition_changed || comment_changed) {
                        return Err(DdlError::Unsupported(
                            "MySQL changes a generated column only with its expression; change it in a query".to_string(),
                        ));
                    }
                    if definition_changed || comment_changed {
                        // CHANGE COLUMN restates the whole definition with the
                        // (possibly new) name, so one statement covers it all.
                        statements.push(format!(
                            "ALTER TABLE {target} CHANGE COLUMN {} {}",
                            quoted(&from.name),
                            column_definition(db_type, to)
                        ));
                    } else {
                        statements.push(rename());
                    }
                }
                DatabaseType::MsSql => {
                    if default_changed {
                        return Err(DdlError::Unsupported(
                            "SQL Server stores defaults as named constraints; change the default in a query instead".to_string(),
                        ));
                    }
                    if renamed {
                        statements.push(rename());
                    }
                    if type_changed || null_changed {
                        statements.push(format!(
                            "ALTER TABLE {target} ALTER COLUMN {column} {} {}",
                            to.data_type.trim(),
                            if to.nullable { "NULL" } else { "NOT NULL" }
                        ));
                    }
                    if comment_changed {
                        statements.push(comment()?);
                    }
                }
                DatabaseType::ClickHouse => {
                    if renamed {
                        statements.push(rename());
                    }
                    if definition_changed {
                        // The definition carries the comment along.
                        statements.push(format!(
                            "ALTER TABLE {target} MODIFY COLUMN {}",
                            column_definition(db_type, to)
                        ));
                    } else if comment_changed {
                        statements.push(comment()?);
                    }
                }
                DatabaseType::SQLite => {
                    // A rebuild from ColumnInfo would silently drop CHECK and
                    // UNIQUE constraints, triggers and partial indexes, so the
                    // only safe in-place change is the rename.
                    if definition_changed {
                        return Err(DdlError::Unsupported(
                            "SQLite cannot change a column's type, nullability or default in place; recreate the table in a query".to_string(),
                        ));
                    }
                    statements.push(rename());
                }
                DatabaseType::Redis => {
                    return Err(DdlError::Unsupported("Redis has no columns".to_string()));
                }
            }
            Ok(statements)
        }
    }
}

fn rename_column_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    from: &str,
    to: &str,
) -> String {
    let dialect = db_type.dialect();
    match db_type {
        DatabaseType::MsSql => {
            let mut path = String::new();
            if let Some(schema) = schema.filter(|schema| !schema.is_empty()) {
                path.push_str(schema);
                path.push('.');
            }
            path.push_str(table);
            path.push('.');
            path.push_str(from.trim());
            format!(
                "EXEC sp_rename '{}', '{}', 'COLUMN'",
                path.replace('\'', "''"),
                to.trim().replace('\'', "''")
            )
        }
        _ => format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            dialect.quote_qualified(schema, table),
            dialect.quote_identifier(from.trim()),
            dialect.quote_identifier(to.trim())
        ),
    }
}

/// Statements for `operation` on the indexes of `table`.
pub fn index_operation_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    operation: &IndexOperation,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    if !dialect.supports_column_ddl() {
        return Err(DdlError::Unsupported(format!(
            "{} has no indexes",
            db_type.as_str()
        )));
    }
    let target = dialect.quote_qualified(schema, table);
    match operation {
        IndexOperation::Create(spec) => {
            require_name(&spec.name, "index")?;
            if spec.columns.is_empty() {
                return Err(DdlError::Invalid(
                    "an index needs at least one column".to_string(),
                ));
            }
            let method = spec
                .method
                .as_deref()
                .map(str::trim)
                .filter(|method| !method.is_empty());
            let condition = spec
                .condition
                .as_deref()
                .map(str::trim)
                .filter(|condition| !condition.is_empty());
            if spec.nulls_not_distinct {
                if !dialect.has_nulls_not_distinct() {
                    return Err(DdlError::Unsupported(format!(
                        "{} has no NULLS NOT DISTINCT",
                        db_type.as_str()
                    )));
                }
                if !spec.unique {
                    return Err(DdlError::Invalid(
                        "NULLS NOT DISTINCT applies to unique indexes only".to_string(),
                    ));
                }
            }
            if condition.is_some() && !dialect.supports_partial_indexes() {
                return Err(DdlError::Unsupported(format!(
                    "{} has no partial indexes",
                    db_type.as_str()
                )));
            }
            if method.is_some() && dialect.index_methods().is_empty() {
                return Err(DdlError::Unsupported(format!(
                    "{} takes no index method",
                    db_type.as_str()
                )));
            }
            let column_list = spec
                .columns
                .iter()
                .map(|column| dialect.quote_identifier(column.trim()))
                .collect::<Vec<_>>()
                .join(", ");
            let index_name = dialect.quote_identifier(spec.name.trim());
            Ok(vec![match db_type {
                DatabaseType::ClickHouse => {
                    if spec.unique {
                        return Err(DdlError::Unsupported(
                            "ClickHouse has no unique indexes".to_string(),
                        ));
                    }
                    // Only data-skipping indexes can be added after the fact;
                    // minmax is the cheapest general-purpose one.
                    format!(
                        "ALTER TABLE {target} ADD INDEX {index_name} ({column_list}) TYPE {} GRANULARITY 1",
                        method.unwrap_or("minmax")
                    )
                }
                _ => {
                    let mut sql = format!(
                        "CREATE {}INDEX {index_name} ON {target}",
                        if spec.unique { "UNIQUE " } else { "" }
                    );
                    if let Some(method) = method {
                        sql.push_str(&format!(" USING {method}"));
                    }
                    sql.push_str(&format!(" ({column_list})"));
                    if spec.nulls_not_distinct {
                        sql.push_str(" NULLS NOT DISTINCT");
                    }
                    if let Some(condition) = condition {
                        sql.push_str(&format!(" WHERE {condition}"));
                    }
                    sql
                }
            }])
        }
        IndexOperation::Drop { name } => {
            require_name(name, "index")?;
            let index_name = dialect.quote_identifier(name.trim());
            Ok(vec![match db_type {
                // Indexes are schema-scoped objects here, not table-scoped.
                DatabaseType::PostgreSQL
                | DatabaseType::CockroachDb
                | DatabaseType::SQLite
                | DatabaseType::DuckDb => {
                    format!(
                        "DROP INDEX {}",
                        dialect.quote_qualified(schema, name.trim())
                    )
                }
                DatabaseType::MySQL | DatabaseType::MsSql => {
                    format!("DROP INDEX {index_name} ON {target}")
                }
                DatabaseType::ClickHouse => format!("ALTER TABLE {target} DROP INDEX {index_name}"),
                DatabaseType::Redis => {
                    return Err(DdlError::Unsupported("Redis has no indexes".to_string()));
                }
            }])
        }
    }
}

/// `[CONSTRAINT name] FOREIGN KEY (...) REFERENCES ... [ON ...]`, the part
/// of a key both `ALTER TABLE ... ADD` and `CREATE TABLE` spell alike. A
/// referenced table without a schema is in the table's own.
fn foreign_key_clause(
    db_type: DatabaseType,
    schema: Option<&str>,
    operation: &ForeignKeyOperation,
) -> Result<String, DdlError> {
    let ForeignKeyOperation::Create {
        name,
        columns,
        referenced_schema,
        referenced_table,
        referenced_columns,
        on_update,
        on_delete,
    } = operation
    else {
        return Err(DdlError::Invalid("not a new foreign key".to_string()));
    };
    let dialect = db_type.dialect();
    let column_list = |columns: &[String]| {
        columns
            .iter()
            .map(|column| dialect.quote_identifier(column.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    require_name(referenced_table, "referenced table")?;
    if columns.is_empty() {
        return Err(DdlError::Invalid(
            "a foreign key needs at least one column".to_string(),
        ));
    }
    if columns.len() != referenced_columns.len() {
        return Err(DdlError::Invalid(
            "each column needs one referenced column".to_string(),
        ));
    }
    if columns
        .iter()
        .chain(referenced_columns)
        .any(|column| column.trim().is_empty())
    {
        return Err(DdlError::Invalid(
            "column name must not be empty".to_string(),
        ));
    }
    if db_type == DatabaseType::MsSql
        && (*on_update == ReferentialAction::Restrict || *on_delete == ReferentialAction::Restrict)
    {
        return Err(DdlError::Unsupported(
            "SQL Server has no RESTRICT; NO ACTION refuses the change the same way".to_string(),
        ));
    }
    let mut sql = String::new();
    if let Some(name) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        sql.push_str(&format!("CONSTRAINT {} ", dialect.quote_identifier(name)));
    }
    let referenced_schema = referenced_schema
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(schema);
    sql.push_str(&format!(
        "FOREIGN KEY ({}) REFERENCES {} ({})",
        column_list(columns),
        dialect.quote_qualified(referenced_schema, referenced_table.trim()),
        column_list(referenced_columns)
    ));
    // NO ACTION is every backend's default, so leave it out.
    for (clause, action) in [("UPDATE", on_update), ("DELETE", on_delete)] {
        if *action != ReferentialAction::NoAction {
            sql.push_str(&format!(" ON {clause} {}", action.sql()));
        }
    }
    Ok(sql)
}

/// A table to create: its columns in order, the primary key from the
/// columns marked as part of it, then its indexes and foreign keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewTable {
    pub columns: Vec<ColumnSpec>,
    pub indexes: Vec<IndexSpec>,
    /// `ForeignKeyOperation::Create` each.
    pub foreign_keys: Vec<ForeignKeyOperation>,
}

/// The statements that create `table`: the `CREATE TABLE` with its columns,
/// primary key and foreign keys, then its indexes and the column comments
/// the backend keeps apart. Foreign keys are declared inline because SQLite
/// and DuckDB cannot add one later. ClickHouse orders a MergeTree table by
/// its primary key, or by nothing without one.
pub fn create_table_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    new_table: &NewTable,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    if !dialect.supports_column_ddl() {
        return Err(DdlError::Unsupported(format!(
            "{} has no tables to create",
            db_type.as_str()
        )));
    }
    require_name(table, "table")?;
    if new_table.columns.is_empty() {
        return Err(DdlError::Invalid(
            "a table needs at least one column".to_string(),
        ));
    }
    let mut names = std::collections::HashSet::new();
    for spec in &new_table.columns {
        validate_spec(spec)?;
        if !names.insert(spec.name.trim().to_lowercase()) {
            return Err(DdlError::Invalid(format!(
                "column \"{}\" is listed twice",
                spec.name.trim()
            )));
        }
        if spec.trimmed_comment().is_some() && !dialect.supports_comments() {
            return Err(unsupported_comments(db_type));
        }
    }
    if !new_table.foreign_keys.is_empty() && !dialect.supports_foreign_keys() {
        return Err(DdlError::Unsupported(format!(
            "{} has no foreign keys",
            db_type.as_str()
        )));
    }

    let target = dialect.quote_qualified(schema, table);
    let primary_key: Vec<String> = new_table
        .columns
        .iter()
        .filter(|spec| spec.primary_key)
        .map(|spec| dialect.quote_identifier(spec.name.trim()))
        .collect();
    let mut lines: Vec<String> = new_table
        .columns
        .iter()
        .map(|spec| column_definition(db_type, spec))
        .collect();
    if !primary_key.is_empty() && db_type != DatabaseType::ClickHouse {
        lines.push(format!("PRIMARY KEY ({})", primary_key.join(", ")));
    }
    for foreign_key in &new_table.foreign_keys {
        lines.push(foreign_key_clause(db_type, schema, foreign_key)?);
    }
    let mut create = format!("CREATE TABLE {target} (\n    {}\n)", lines.join(",\n    "));
    if db_type == DatabaseType::ClickHouse {
        let order_by = if primary_key.is_empty() {
            "tuple()".to_string()
        } else {
            format!("({})", primary_key.join(", "))
        };
        create.push_str(&format!("\nENGINE = MergeTree\nORDER BY {order_by}"));
    }

    let mut statements = vec![create];
    for index in &new_table.indexes {
        statements.extend(index_operation_sql(
            db_type,
            schema,
            table,
            &IndexOperation::Create(index.clone()),
        )?);
    }
    if !matches!(db_type, DatabaseType::MySQL | DatabaseType::ClickHouse) {
        for spec in &new_table.columns {
            if let Some(comment) = spec.trimmed_comment() {
                statements.push(column_comment_sql(
                    db_type,
                    schema,
                    table,
                    &spec.name,
                    Some(comment),
                )?);
            }
        }
    }
    Ok(statements)
}

/// Statements for `operation` on the foreign keys of `table`.
pub fn foreign_key_operation_sql(
    db_type: DatabaseType,
    schema: Option<&str>,
    table: &str,
    operation: &ForeignKeyOperation,
) -> Result<Vec<String>, DdlError> {
    let dialect = db_type.dialect();
    if !dialect.supports_foreign_key_ddl() {
        return Err(DdlError::Unsupported(match db_type {
            DatabaseType::SQLite | DatabaseType::DuckDb => format!(
                "{} cannot add or drop a foreign key in place; recreate the table in a query",
                db_type.as_str()
            ),
            _ => format!("{} has no foreign keys", db_type.as_str()),
        }));
    }
    let target = dialect.quote_qualified(schema, table);
    match operation {
        ForeignKeyOperation::Create { .. } => Ok(vec![format!(
            "ALTER TABLE {target} ADD {}",
            foreign_key_clause(db_type, schema, operation)?
        )]),
        ForeignKeyOperation::Drop { name } => {
            require_name(name, "foreign key")?;
            let name = dialect.quote_identifier(name.trim());
            Ok(vec![match db_type {
                DatabaseType::MySQL => format!("ALTER TABLE {target} DROP FOREIGN KEY {name}"),
                _ => format!("ALTER TABLE {target} DROP CONSTRAINT {name}"),
            }])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_and_truncate_per_backend() {
        let sql = |db_type, op| {
            table_operation_sql(db_type, EntityType::Table, Some("app"), "order", &op)
        };
        assert_eq!(
            sql(DatabaseType::PostgreSQL, TableOperation::Drop).as_deref(),
            Some("DROP TABLE \"app\".\"order\"")
        );
        assert_eq!(
            sql(DatabaseType::MySQL, TableOperation::Truncate).as_deref(),
            Some("TRUNCATE TABLE `app`.`order`")
        );
        assert_eq!(
            sql(DatabaseType::MsSql, TableOperation::Drop).as_deref(),
            Some("DROP TABLE [app].[order]")
        );
        assert_eq!(
            table_operation_sql(
                DatabaseType::SQLite,
                EntityType::Table,
                None,
                "t",
                &TableOperation::Truncate
            )
            .as_deref(),
            Some("DELETE FROM \"t\"")
        );
        assert!(sql(DatabaseType::Redis, TableOperation::Drop).is_none());
        assert!(
            table_operation_sql(
                DatabaseType::PostgreSQL,
                EntityType::View,
                None,
                "v",
                &TableOperation::Truncate
            )
            .is_none()
        );
    }

    #[test]
    fn rename_per_backend() {
        let rename = TableOperation::Rename {
            new_name: "orders".into(),
        };
        assert_eq!(
            table_operation_sql(
                DatabaseType::PostgreSQL,
                EntityType::Table,
                Some("app"),
                "order",
                &rename
            )
            .as_deref(),
            Some("ALTER TABLE \"app\".\"order\" RENAME TO \"orders\"")
        );
        assert_eq!(
            table_operation_sql(
                DatabaseType::MsSql,
                EntityType::Table,
                Some("dbo"),
                "order",
                &rename
            )
            .as_deref(),
            Some("EXEC sp_rename 'dbo.order', 'orders'")
        );
        assert_eq!(
            table_operation_sql(
                DatabaseType::ClickHouse,
                EntityType::Table,
                Some("db"),
                "order",
                &rename
            )
            .as_deref(),
            Some("RENAME TABLE `db`.`order` TO `db`.`orders`")
        );
        assert!(
            table_operation_sql(
                DatabaseType::SQLite,
                EntityType::Table,
                None,
                "t",
                &TableOperation::Rename {
                    new_name: "  ".into()
                }
            )
            .is_none()
        );
    }

    #[test]
    fn identifiers_are_escaped() {
        assert_eq!(
            quote_identifier(DatabaseType::PostgreSQL, "we\"ird"),
            "\"we\"\"ird\""
        );
        assert_eq!(quote_identifier(DatabaseType::MsSql, "a]b"), "[a]]b]");
    }

    fn spec(name: &str, data_type: &str, nullable: bool, default: Option<&str>) -> ColumnSpec {
        ColumnSpec {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable,
            default: default.map(str::to_string),
            ..ColumnSpec::default()
        }
    }

    #[test]
    fn column_positions_per_backend() {
        let first = ColumnOperation::Add {
            column: spec("age", "int", true, None),
            position: Some(ColumnPosition::First),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "user", &first),
            Ok(vec![
                "ALTER TABLE `user` ADD COLUMN `age` int FIRST".to_string()
            ])
        );
        let after = ColumnOperation::Add {
            column: ColumnSpec {
                comment: Some("Years".to_string()),
                ..spec("age", "UInt8", false, None)
            },
            position: Some(ColumnPosition::After("name".to_string())),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "user", &after),
            Ok(vec![
                "ALTER TABLE `user` ADD COLUMN `age` UInt8 COMMENT 'Years' AFTER `name`"
                    .to_string()
            ])
        );
        for db_type in [
            DatabaseType::PostgreSQL,
            DatabaseType::CockroachDb,
            DatabaseType::SQLite,
            DatabaseType::MsSql,
            DatabaseType::DuckDb,
        ] {
            assert!(
                matches!(
                    column_operation_sql(db_type, None, "user", &first),
                    Err(DdlError::Unsupported(_))
                ),
                "{db_type:?}"
            );
        }

        // MySQL restates the column, attributes and comment included.
        let id = ColumnSpec {
            attributes: Some("AUTO_INCREMENT".to_string()),
            comment: Some("Key".to_string()),
            ..spec("id", "bigint unsigned", false, None)
        };
        let move_id = ColumnOperation::Move {
            column: id.clone(),
            position: ColumnPosition::After("email".to_string()),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, Some("app"), "user", &move_id),
            Ok(vec![
                "ALTER TABLE `app`.`user` MODIFY COLUMN `id` bigint unsigned NOT NULL AUTO_INCREMENT COMMENT 'Key' AFTER `email`"
                    .to_string()
            ])
        );
        // ClickHouse keeps everything but the type, which it must be given.
        let move_note = ColumnOperation::Move {
            column: ColumnSpec {
                default: Some("''".to_string()),
                ..spec("note", "String", true, None)
            },
            position: ColumnPosition::First,
        };
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "t", &move_note),
            Ok(vec![
                "ALTER TABLE `t` MODIFY COLUMN `note` Nullable(String) FIRST".to_string()
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &move_note),
            Err(DdlError::Unsupported(_))
        ));
        let generated = ColumnOperation::Move {
            column: ColumnSpec {
                generated: true,
                ..id.clone()
            },
            position: ColumnPosition::First,
        };
        assert!(matches!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &generated),
            Err(DdlError::Unsupported(_))
        ));
        let after_itself = ColumnOperation::Move {
            column: id,
            position: ColumnPosition::After("id".to_string()),
        };
        assert!(matches!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &after_itself),
            Err(DdlError::Invalid(_))
        ));
    }

    #[test]
    fn add_column_per_backend() {
        let add = ColumnOperation::Add {
            column: spec("age", "integer", false, Some("0")),
            position: None,
        };
        let sql = |db_type| column_operation_sql(db_type, Some("app"), "user", &add);
        assert_eq!(
            sql(DatabaseType::PostgreSQL),
            Ok(vec![
                "ALTER TABLE \"app\".\"user\" ADD COLUMN \"age\" integer NOT NULL DEFAULT 0"
                    .to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::MySQL),
            Ok(vec![
                "ALTER TABLE `app`.`user` ADD COLUMN `age` integer NOT NULL DEFAULT 0".to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::MsSql),
            Ok(vec![
                "ALTER TABLE [app].[user] ADD [age] integer NOT NULL DEFAULT 0".to_string()
            ])
        );
        let nullable = ColumnOperation::Add {
            column: spec("note", "String", true, None),
            position: None,
        };
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "t", &nullable),
            Ok(vec![
                "ALTER TABLE `t` ADD COLUMN `note` Nullable(String)".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::SQLite, None, "t", &nullable),
            Ok(vec![
                "ALTER TABLE \"t\" ADD COLUMN \"note\" String".to_string()
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::Redis, None, "t", &nullable),
            Err(DdlError::Unsupported(_))
        ));
        assert!(matches!(
            column_operation_sql(
                DatabaseType::PostgreSQL,
                None,
                "t",
                &ColumnOperation::Add {
                    column: spec("", "int", true, None),
                    position: None,
                }
            ),
            Err(DdlError::Invalid(_))
        ));
    }

    #[test]
    fn rename_and_drop_column_per_backend() {
        let rename = ColumnOperation::Rename {
            from: "old".to_string(),
            to: "new".to_string(),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &rename),
            Ok(vec![
                "ALTER TABLE \"app\".\"t\" RENAME COLUMN \"old\" TO \"new\"".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &rename),
            Ok(vec![
                "EXEC sp_rename 'dbo.t.old', 'new', 'COLUMN'".to_string()
            ])
        );
        let drop = ColumnOperation::Drop {
            name: "old".to_string(),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &drop),
            Ok(vec!["ALTER TABLE `t` DROP COLUMN `old`".to_string()])
        );
    }

    #[test]
    fn alter_column_per_backend() {
        let alter = ColumnOperation::Alter {
            from: spec("qty", "int", false, Some("0")),
            to: spec("quantity", "bigint", true, None),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &alter),
            Ok(vec![
                "ALTER TABLE \"t\" RENAME COLUMN \"qty\" TO \"quantity\"".to_string(),
                "ALTER TABLE \"t\" ALTER COLUMN \"quantity\" TYPE bigint, ALTER COLUMN \"quantity\" DROP NOT NULL, ALTER COLUMN \"quantity\" DROP DEFAULT".to_string(),
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &alter),
            Ok(vec![
                "ALTER TABLE `t` CHANGE COLUMN `qty` `quantity` bigint".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "t", &alter),
            Ok(vec![
                "ALTER TABLE `t` RENAME COLUMN `qty` TO `quantity`".to_string(),
                "ALTER TABLE `t` MODIFY COLUMN `quantity` Nullable(bigint)".to_string(),
            ])
        );
        // SQL Server keeps a default as a named constraint.
        assert!(matches!(
            column_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &alter),
            Err(DdlError::Unsupported(_))
        ));
        let widen = ColumnOperation::Alter {
            from: spec("qty", "int", false, None),
            to: spec("quantity", "bigint", true, None),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &widen),
            Ok(vec![
                "EXEC sp_rename 'dbo.t.qty', 'quantity', 'COLUMN'".to_string(),
                "ALTER TABLE [dbo].[t] ALTER COLUMN [quantity] bigint NULL".to_string(),
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::SQLite, None, "t", &widen),
            Err(DdlError::Unsupported(_))
        ));

        // Only what changed is changed.
        let default_only = ColumnOperation::Alter {
            from: spec("qty", "int", false, None),
            to: spec("qty", "int", false, Some("1")),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &default_only),
            Ok(vec![
                "ALTER TABLE \"t\" ALTER COLUMN \"qty\" SET DEFAULT 1".to_string(),
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::DuckDb, None, "t", &default_only),
            Ok(vec![
                "ALTER TABLE \"t\" ALTER COLUMN \"qty\" SET DEFAULT 1".to_string(),
            ])
        );
        // A rename alone is the one change SQLite makes in place.
        let rename_only = ColumnOperation::Alter {
            from: spec("qty", "int", false, None),
            to: spec("quantity", "int", false, None),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::SQLite, None, "t", &rename_only),
            Ok(vec![
                "ALTER TABLE \"t\" RENAME COLUMN \"qty\" TO \"quantity\"".to_string(),
            ])
        );
        let unchanged = ColumnOperation::Alter {
            from: spec("qty", "int", false, None),
            to: spec("qty", "int", false, None),
        };
        assert!(matches!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &unchanged),
            Err(DdlError::Invalid(_))
        ));
    }

    #[test]
    fn mysql_restates_column_attributes_and_refuses_generated_columns() {
        let id = ColumnSpec {
            attributes: Some("auto_increment".to_string()),
            ..spec("id", "int(10) unsigned", false, None)
        };
        let comment = ColumnOperation::Alter {
            from: id.clone(),
            to: ColumnSpec {
                comment: Some("Row id".to_string()),
                ..id
            },
        };
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &comment),
            Ok(vec![
                "ALTER TABLE `t` CHANGE COLUMN `id` `id` int(10) unsigned NOT NULL auto_increment COMMENT 'Row id'".to_string()
            ])
        );
        let generated = ColumnSpec {
            generated: true,
            ..spec("twice", "int(11)", true, None)
        };
        let change = |to: ColumnSpec| ColumnOperation::Alter {
            from: generated.clone(),
            to,
        };
        assert!(matches!(
            column_operation_sql(
                DatabaseType::MySQL,
                None,
                "t",
                &change(ColumnSpec {
                    comment: Some("x".to_string()),
                    ..generated.clone()
                })
            ),
            Err(DdlError::Unsupported(_))
        ));
        // A rename alone needs no definition.
        assert_eq!(
            column_operation_sql(
                DatabaseType::MySQL,
                None,
                "t",
                &change(ColumnSpec {
                    name: "double".to_string(),
                    ..generated.clone()
                })
            ),
            Ok(vec![
                "ALTER TABLE `t` RENAME COLUMN `twice` TO `double`".to_string()
            ])
        );
    }

    #[test]
    fn column_comments_per_backend() {
        let commented = |comment: Option<&str>| ColumnSpec {
            comment: comment.map(str::to_string),
            ..spec("code", "text", true, None)
        };
        let add = ColumnOperation::Add {
            column: commented(Some("ISO 4217's code")),
            position: None,
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &add),
            Ok(vec![
                "ALTER TABLE \"app\".\"t\" ADD COLUMN \"code\" text".to_string(),
                "COMMENT ON COLUMN \"app\".\"t\".\"code\" IS 'ISO 4217''s code'".to_string(),
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &add),
            Ok(vec![
                "ALTER TABLE `t` ADD COLUMN `code` text COMMENT 'ISO 4217''s code'".to_string()
            ])
        );
        assert!(matches!(
            column_operation_sql(DatabaseType::SQLite, None, "t", &add),
            Err(DdlError::Unsupported(_))
        ));

        let set = ColumnOperation::Alter {
            from: commented(None),
            to: commented(Some("Currency")),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::ClickHouse, None, "t", &set),
            Ok(vec![
                "ALTER TABLE `t` COMMENT COLUMN `code` 'Currency'".to_string()
            ])
        );
        // MySQL restates the definition, which keeps the comment.
        assert_eq!(
            column_operation_sql(DatabaseType::MySQL, None, "t", &set),
            Ok(vec![
                "ALTER TABLE `t` CHANGE COLUMN `code` `code` text COMMENT 'Currency'".to_string()
            ])
        );
        assert_eq!(
            column_operation_sql(DatabaseType::MsSql, Some("sales"), "t", &set),
            Ok(vec![
                "IF EXISTS (SELECT 1 FROM sys.extended_properties WHERE class = 1 AND name = N'MS_Description' AND major_id = OBJECT_ID(N'[sales].[t]') AND minor_id = COLUMNPROPERTY(OBJECT_ID(N'[sales].[t]'), N'code', 'ColumnId')) EXEC sp_updateextendedproperty @name = N'MS_Description', @value = N'Currency', @level0type = N'SCHEMA', @level0name = N'sales', @level1type = N'TABLE', @level1name = N't', @level2type = N'COLUMN', @level2name = N'code' ELSE EXEC sp_addextendedproperty @name = N'MS_Description', @value = N'Currency', @level0type = N'SCHEMA', @level0name = N'sales', @level1type = N'TABLE', @level1name = N't', @level2type = N'COLUMN', @level2name = N'code'".to_string()
            ])
        );
        let clear = ColumnOperation::Alter {
            from: commented(Some("Currency")),
            to: commented(Some("  ")),
        };
        assert_eq!(
            column_operation_sql(DatabaseType::PostgreSQL, None, "t", &clear),
            Ok(vec!["COMMENT ON COLUMN \"t\".\"code\" IS NULL".to_string()])
        );
    }

    #[test]
    fn table_comment_per_backend() {
        let set = TableChange::Comment(Some("Orders".to_string()));
        let sql =
            |db_type, change: &TableChange| table_change_sql(db_type, Some("app"), "t", change);
        assert_eq!(
            sql(DatabaseType::PostgreSQL, &set),
            Ok(vec![
                "COMMENT ON TABLE \"app\".\"t\" IS 'Orders'".to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::MySQL, &set),
            Ok(vec!["ALTER TABLE `app`.`t` COMMENT = 'Orders'".to_string()])
        );
        assert_eq!(
            sql(DatabaseType::ClickHouse, &set),
            Ok(vec![
                "ALTER TABLE `app`.`t` MODIFY COMMENT 'Orders'".to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::MsSql, &TableChange::Comment(None)),
            Ok(vec![
                "IF EXISTS (SELECT 1 FROM sys.extended_properties WHERE class = 1 AND name = N'MS_Description' AND major_id = OBJECT_ID(N'[app].[t]') AND minor_id = 0) EXEC sp_dropextendedproperty @name = N'MS_Description', @level0type = N'SCHEMA', @level0name = N'app', @level1type = N'TABLE', @level1name = N't'".to_string()
            ])
        );
        assert_eq!(
            sql(DatabaseType::DuckDb, &TableChange::Comment(None)),
            Ok(vec!["COMMENT ON TABLE \"app\".\"t\" IS NULL".to_string()])
        );
        assert!(matches!(
            sql(DatabaseType::SQLite, &set),
            Err(DdlError::Unsupported(_))
        ));
    }

    #[test]
    fn index_operations_per_backend() {
        let create = IndexOperation::Create(IndexSpec {
            name: "t_a_b".to_string(),
            columns: vec!["a".to_string(), "b".to_string()],
            unique: true,
            ..IndexSpec::default()
        });
        assert_eq!(
            index_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &create),
            Ok(vec![
                "CREATE UNIQUE INDEX \"t_a_b\" ON \"app\".\"t\" (\"a\", \"b\")".to_string()
            ])
        );
        assert_eq!(
            index_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &create),
            Ok(vec![
                "CREATE UNIQUE INDEX [t_a_b] ON [dbo].[t] ([a], [b])".to_string()
            ])
        );
        assert!(matches!(
            index_operation_sql(DatabaseType::ClickHouse, None, "t", &create),
            Err(DdlError::Unsupported(_))
        ));
        let skipping = IndexOperation::Create(IndexSpec {
            name: "idx".to_string(),
            columns: vec!["a".to_string()],
            ..IndexSpec::default()
        });
        assert_eq!(
            index_operation_sql(DatabaseType::ClickHouse, None, "t", &skipping),
            Ok(vec![
                "ALTER TABLE `t` ADD INDEX `idx` (`a`) TYPE minmax GRANULARITY 1".to_string()
            ])
        );
        let drop = IndexOperation::Drop {
            name: "idx".to_string(),
        };
        assert_eq!(
            index_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &drop),
            Ok(vec!["DROP INDEX \"app\".\"idx\"".to_string()])
        );
        assert_eq!(
            index_operation_sql(DatabaseType::MySQL, None, "t", &drop),
            Ok(vec!["DROP INDEX `idx` ON `t`".to_string()])
        );
        assert_eq!(
            index_operation_sql(DatabaseType::ClickHouse, None, "t", &drop),
            Ok(vec!["ALTER TABLE `t` DROP INDEX `idx`".to_string()])
        );
        assert!(matches!(
            index_operation_sql(
                DatabaseType::SQLite,
                None,
                "t",
                &IndexOperation::Create(IndexSpec {
                    name: "i".to_string(),
                    ..IndexSpec::default()
                })
            ),
            Err(DdlError::Invalid(_))
        ));
    }

    #[test]
    fn create_table_per_backend() {
        let new_table = |db_type: DatabaseType| {
            let (key_type, attributes) = db_type.dialect().new_table_key();
            NewTable {
                columns: vec![
                    ColumnSpec {
                        name: "id".to_string(),
                        data_type: key_type.to_string(),
                        attributes: attributes.map(str::to_string),
                        primary_key: true,
                        ..ColumnSpec::default()
                    },
                    ColumnSpec {
                        name: "customer_id".to_string(),
                        data_type: "integer".to_string(),
                        nullable: false,
                        comment: Some("Who ordered".to_string()),
                        ..ColumnSpec::default()
                    },
                ],
                indexes: vec![IndexSpec {
                    name: "orders_customer_idx".to_string(),
                    columns: vec!["customer_id".to_string()],
                    ..IndexSpec::default()
                }],
                foreign_keys: vec![ForeignKeyOperation::Create {
                    name: None,
                    columns: vec!["customer_id".to_string()],
                    referenced_schema: None,
                    referenced_table: "customers".to_string(),
                    referenced_columns: vec!["id".to_string()],
                    on_update: ReferentialAction::NoAction,
                    on_delete: ReferentialAction::Cascade,
                }],
            }
        };
        assert_eq!(
            create_table_sql(DatabaseType::PostgreSQL, Some("app"), "orders", &new_table(DatabaseType::PostgreSQL)),
            Ok(vec![
                "CREATE TABLE \"app\".\"orders\" (\n    \"id\" bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n    \"customer_id\" integer NOT NULL,\n    PRIMARY KEY (\"id\"),\n    FOREIGN KEY (\"customer_id\") REFERENCES \"app\".\"customers\" (\"id\") ON DELETE CASCADE\n)".to_string(),
                "CREATE INDEX \"orders_customer_idx\" ON \"app\".\"orders\" (\"customer_id\")".to_string(),
                "COMMENT ON COLUMN \"app\".\"orders\".\"customer_id\" IS 'Who ordered'".to_string(),
            ])
        );
        assert_eq!(
            create_table_sql(DatabaseType::MySQL, None, "orders", &new_table(DatabaseType::MySQL)),
            Ok(vec![
                "CREATE TABLE `orders` (\n    `id` bigint unsigned NOT NULL AUTO_INCREMENT,\n    `customer_id` integer NOT NULL COMMENT 'Who ordered',\n    PRIMARY KEY (`id`),\n    FOREIGN KEY (`customer_id`) REFERENCES `customers` (`id`) ON DELETE CASCADE\n)".to_string(),
                "CREATE INDEX `orders_customer_idx` ON `orders` (`customer_id`)".to_string(),
            ])
        );
        // SQLite takes a foreign key only here, as it cannot add one later.
        let sqlite = NewTable {
            columns: new_table(DatabaseType::SQLite)
                .columns
                .into_iter()
                .map(|spec| ColumnSpec {
                    comment: None,
                    ..spec
                })
                .collect(),
            ..new_table(DatabaseType::SQLite)
        };
        assert_eq!(
            create_table_sql(DatabaseType::SQLite, None, "orders", &sqlite).map(|sql| sql[0].clone()),
            Ok("CREATE TABLE \"orders\" (\n    \"id\" INTEGER NOT NULL,\n    \"customer_id\" integer NOT NULL,\n    PRIMARY KEY (\"id\"),\n    FOREIGN KEY (\"customer_id\") REFERENCES \"customers\" (\"id\") ON DELETE CASCADE\n)".to_string())
        );
        assert!(matches!(
            create_table_sql(
                DatabaseType::SQLite,
                None,
                "orders",
                &new_table(DatabaseType::SQLite)
            ),
            Err(DdlError::Unsupported(_))
        ));
        // ClickHouse has no foreign keys, and orders by its key.
        assert!(matches!(
            create_table_sql(
                DatabaseType::ClickHouse,
                None,
                "orders",
                &new_table(DatabaseType::ClickHouse)
            ),
            Err(DdlError::Unsupported(_))
        ));
        let clickhouse = NewTable {
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            ..new_table(DatabaseType::ClickHouse)
        };
        assert_eq!(
            create_table_sql(DatabaseType::ClickHouse, None, "orders", &clickhouse),
            Ok(vec![
                "CREATE TABLE `orders` (\n    `id` UInt64,\n    `customer_id` integer COMMENT 'Who ordered'\n)\nENGINE = MergeTree\nORDER BY (`id`)".to_string(),
            ])
        );
        assert!(matches!(
            create_table_sql(DatabaseType::PostgreSQL, None, " ", &clickhouse),
            Err(DdlError::Invalid(_))
        ));
        assert!(matches!(
            create_table_sql(DatabaseType::PostgreSQL, None, "t", &NewTable::default()),
            Err(DdlError::Invalid(_))
        ));
    }

    #[test]
    fn index_options_per_backend() {
        let unique_email = |nulls_not_distinct| IndexSpec {
            name: "users_email_key".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            nulls_not_distinct,
            ..IndexSpec::default()
        };
        assert_eq!(
            index_operation_sql(
                DatabaseType::PostgreSQL,
                None,
                "users",
                &IndexOperation::Create(unique_email(true))
            ),
            Ok(vec![
                "CREATE UNIQUE INDEX \"users_email_key\" ON \"users\" (\"email\") NULLS NOT DISTINCT".to_string()
            ])
        );
        assert!(matches!(
            index_operation_sql(
                DatabaseType::MySQL,
                None,
                "users",
                &IndexOperation::Create(unique_email(true))
            ),
            Err(DdlError::Unsupported(_))
        ));
        assert!(matches!(
            index_operation_sql(
                DatabaseType::PostgreSQL,
                None,
                "users",
                &IndexOperation::Create(IndexSpec {
                    unique: false,
                    ..unique_email(true)
                })
            ),
            Err(DdlError::Invalid(_))
        ));

        let partial = IndexSpec {
            name: "open_idx".to_string(),
            columns: vec!["status".to_string()],
            method: Some("hash".to_string()),
            condition: Some("shipped_at IS NULL".to_string()),
            ..IndexSpec::default()
        };
        assert_eq!(
            index_operation_sql(
                DatabaseType::PostgreSQL,
                Some("sales"),
                "orders",
                &IndexOperation::Create(partial.clone())
            ),
            Ok(vec![
                "CREATE INDEX \"open_idx\" ON \"sales\".\"orders\" USING hash (\"status\") WHERE shipped_at IS NULL".to_string()
            ])
        );
        let condition_only = IndexSpec {
            method: None,
            ..partial.clone()
        };
        assert_eq!(
            index_operation_sql(
                DatabaseType::SQLite,
                None,
                "orders",
                &IndexOperation::Create(condition_only.clone())
            ),
            Ok(vec![
                "CREATE INDEX \"open_idx\" ON \"orders\" (\"status\") WHERE shipped_at IS NULL"
                    .to_string()
            ])
        );
        assert!(matches!(
            index_operation_sql(
                DatabaseType::MySQL,
                None,
                "orders",
                &IndexOperation::Create(condition_only)
            ),
            Err(DdlError::Unsupported(_))
        ));
        assert_eq!(
            index_operation_sql(
                DatabaseType::ClickHouse,
                None,
                "orders",
                &IndexOperation::Create(IndexSpec {
                    method: Some("bloom_filter".to_string()),
                    condition: None,
                    ..partial
                })
            ),
            Ok(vec![
                "ALTER TABLE `orders` ADD INDEX `open_idx` (`status`) TYPE bloom_filter GRANULARITY 1".to_string()
            ])
        );
    }

    #[test]
    fn foreign_key_operations_per_backend() {
        let create = |name: Option<&str>, on_update, on_delete| ForeignKeyOperation::Create {
            name: name.map(str::to_string),
            columns: vec!["customer_id".to_string(), "region".to_string()],
            referenced_schema: None,
            referenced_table: "customers".to_string(),
            referenced_columns: vec!["id".to_string(), "region".to_string()],
            on_update,
            on_delete,
        };
        let cascade = create(
            Some("orders_customer_fk"),
            ReferentialAction::NoAction,
            ReferentialAction::Cascade,
        );
        assert_eq!(
            foreign_key_operation_sql(DatabaseType::PostgreSQL, Some("app"), "orders", &cascade),
            Ok(vec![
                "ALTER TABLE \"app\".\"orders\" ADD CONSTRAINT \"orders_customer_fk\" FOREIGN KEY (\"customer_id\", \"region\") REFERENCES \"app\".\"customers\" (\"id\", \"region\") ON DELETE CASCADE".to_string()
            ])
        );
        let unnamed = create(
            None,
            ReferentialAction::SetNull,
            ReferentialAction::Restrict,
        );
        assert_eq!(
            foreign_key_operation_sql(DatabaseType::MySQL, None, "orders", &unnamed),
            Ok(vec![
                "ALTER TABLE `orders` ADD FOREIGN KEY (`customer_id`, `region`) REFERENCES `customers` (`id`, `region`) ON UPDATE SET NULL ON DELETE RESTRICT".to_string()
            ])
        );
        assert!(matches!(
            foreign_key_operation_sql(DatabaseType::MsSql, Some("dbo"), "orders", &unnamed),
            Err(DdlError::Unsupported(_))
        ));
        let other_schema = ForeignKeyOperation::Create {
            name: None,
            columns: vec!["a".to_string()],
            referenced_schema: Some("ref".to_string()),
            referenced_table: "r".to_string(),
            referenced_columns: vec!["b".to_string()],
            on_update: ReferentialAction::NoAction,
            on_delete: ReferentialAction::NoAction,
        };
        assert_eq!(
            foreign_key_operation_sql(DatabaseType::MsSql, Some("dbo"), "t", &other_schema),
            Ok(vec![
                "ALTER TABLE [dbo].[t] ADD FOREIGN KEY ([a]) REFERENCES [ref].[r] ([b])"
                    .to_string()
            ])
        );

        let drop = ForeignKeyOperation::Drop {
            name: "fk".to_string(),
        };
        assert_eq!(
            foreign_key_operation_sql(DatabaseType::MySQL, None, "t", &drop),
            Ok(vec!["ALTER TABLE `t` DROP FOREIGN KEY `fk`".to_string()])
        );
        assert_eq!(
            foreign_key_operation_sql(DatabaseType::PostgreSQL, Some("app"), "t", &drop),
            Ok(vec![
                "ALTER TABLE \"app\".\"t\" DROP CONSTRAINT \"fk\"".to_string()
            ])
        );
        for db_type in [DatabaseType::SQLite, DatabaseType::ClickHouse] {
            assert!(matches!(
                foreign_key_operation_sql(db_type, None, "t", &drop),
                Err(DdlError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn foreign_key_create_is_validated() {
        let create = |columns: &[&str], referenced: &[&str], table: &str| {
            foreign_key_operation_sql(
                DatabaseType::PostgreSQL,
                None,
                "t",
                &ForeignKeyOperation::Create {
                    name: None,
                    columns: columns.iter().map(|c| c.to_string()).collect(),
                    referenced_schema: None,
                    referenced_table: table.to_string(),
                    referenced_columns: referenced.iter().map(|c| c.to_string()).collect(),
                    on_update: ReferentialAction::NoAction,
                    on_delete: ReferentialAction::NoAction,
                },
            )
        };
        assert!(matches!(create(&[], &[], "r"), Err(DdlError::Invalid(_))));
        assert!(matches!(
            create(&["a", "b"], &["x"], "r"),
            Err(DdlError::Invalid(_))
        ));
        assert!(matches!(
            create(&["a"], &[" "], "r"),
            Err(DdlError::Invalid(_))
        ));
        assert!(matches!(
            create(&["a"], &["x"], " "),
            Err(DdlError::Invalid(_))
        ));
        assert!(create(&["a"], &["x"], "r").is_ok());
    }
}
