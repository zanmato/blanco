//! The identity a tab, action or dialog carries for "the connection and
//! database it works against". Everything that used to spell out six fields
//! (`connection_id`, `connection_name`, `db_type`, `database_name`,
//! `schema_name`, `environment_type`) carries one of these instead.

use crate::{DatabaseType, Dialect};

/// Environment type for database connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvironmentType {
    #[default]
    Dev = 1,
    Test = 2,
    Prod = 3,
}

impl EnvironmentType {
    pub fn from_i32(value: i32) -> Self {
        match value {
            1 => EnvironmentType::Dev,
            2 => EnvironmentType::Test,
            3 => EnvironmentType::Prod,
            _ => EnvironmentType::Dev, // Default to Dev for invalid values
        }
    }

    pub fn to_i32(self) -> i32 {
        self as i32
    }

    pub fn display_name(self) -> &'static str {
        match self {
            EnvironmentType::Dev => "DEV",
            EnvironmentType::Test => "TEST",
            EnvironmentType::Prod => "PROD",
        }
    }
}

impl std::fmt::Display for EnvironmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionContext {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
}

impl ConnectionContext {
    pub fn dialect(&self) -> Dialect {
        self.db_type.dialect()
    }

    pub fn is_prod(&self) -> bool {
        self.environment_type == Some(EnvironmentType::Prod)
    }

    pub fn with_schema(mut self, schema_name: Option<String>) -> Self {
        self.schema_name = schema_name;
        self
    }

    pub fn with_database(mut self, database_name: String) -> Self {
        self.database_name = database_name;
        self
    }

    /// `schema.table` / `database.table` label for titles and messages, not
    /// SQL. Use [`Dialect::quote_qualified`] for statements.
    pub fn qualified_label(&self, name: &str) -> String {
        match self
            .schema_name
            .as_deref()
            .filter(|schema| !schema.is_empty())
        {
            Some(schema) => format!("{schema}.{name}"),
            None => name.to_string(),
        }
    }
}
