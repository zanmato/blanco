//! The identity a tab, action or dialog carries for "the connection and
//! database it works against". Everything that used to spell out six fields
//! (`connection_id`, `connection_name`, `db_type`, `database_name`,
//! `schema_name`, `environment_type`) carries one of these instead.

use crate::{DatabaseType, Dialect};

/// The environment tag of a connection: a built-in one or one the user
/// defined, identified by the id stored with the connection. Names, colours
/// and whether writes are confirmed come from the [`EnvironmentTag`] registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvironmentType(i32);

impl Default for EnvironmentType {
    fn default() -> Self {
        Self::DEV
    }
}

impl EnvironmentType {
    pub const DEV: Self = Self(1);
    pub const TEST: Self = Self(2);
    pub const PROD: Self = Self(3);
    pub const LOCAL: Self = Self(4);
    pub const STAGING: Self = Self(5);

    /// The built-in tags, in the order they are offered.
    pub const BUILT_IN: [Self; 5] = [
        Self::LOCAL,
        Self::DEV,
        Self::TEST,
        Self::STAGING,
        Self::PROD,
    ];

    /// Ids from here on belong to tags the user defined.
    pub const FIRST_CUSTOM_ID: i32 = 100;

    pub fn from_i32(value: i32) -> Self {
        if value > 0 { Self(value) } else { Self::DEV }
    }

    pub fn to_i32(self) -> i32 {
        self.0
    }

    pub fn is_built_in(self) -> bool {
        Self::BUILT_IN.contains(&self)
    }

    /// The tag's name, "DEV" for a tag that no longer exists.
    pub fn display_name(self) -> String {
        self.tag().name
    }

    /// Whether writes on connections with this tag are confirmed first, as
    /// PROD connections have always been.
    pub fn confirms_writes(self) -> bool {
        self.tag().confirms_writes
    }

    /// The registered tag, or the built-in DEV tag in its place.
    pub fn tag(self) -> EnvironmentTag {
        environment_tags()
            .into_iter()
            .find(|tag| tag.id == self)
            .unwrap_or_else(|| built_in_tag(Self::DEV))
    }
}

impl std::fmt::Display for EnvironmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// An environment tag: what a connection is labelled with in the sidebar,
/// the tab strip and confirmations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentTag {
    pub id: EnvironmentType,
    pub name: String,
    /// One of [`TAG_COLORS`], resolved against the theme by the UI, or a
    /// color of the user's own as hex (`#ff8800`).
    pub color: String,
    /// Writes are confirmed before they run, as on PROD.
    pub confirms_writes: bool,
}

/// The colours a tag can have. They name theme colours, so tags follow the
/// light and dark themes.
pub const TAG_COLORS: [&str; 7] = ["gray", "blue", "cyan", "green", "yellow", "red", "magenta"];

fn built_in_tag(id: EnvironmentType) -> EnvironmentTag {
    let (name, color, confirms_writes) = match id {
        EnvironmentType::LOCAL => ("LOCAL", "gray", false),
        EnvironmentType::TEST => ("TEST", "green", false),
        EnvironmentType::STAGING => ("STAGING", "yellow", false),
        EnvironmentType::PROD => ("PROD", "red", true),
        _ => ("DEV", "blue", false),
    };
    EnvironmentTag {
        id,
        name: name.to_string(),
        color: color.to_string(),
        confirms_writes,
    }
}

/// The tags the user defined. Kept process-wide so the MCP server, which runs
/// off the UI thread, sees the same tags as the window.
static CUSTOM_TAGS: std::sync::RwLock<Vec<EnvironmentTag>> = std::sync::RwLock::new(Vec::new());

/// Replace the user-defined tags, after loading or editing them.
pub fn set_custom_environment_tags(tags: Vec<EnvironmentTag>) {
    if let Ok(mut custom) = CUSTOM_TAGS.write() {
        *custom = tags;
    }
}

/// Every tag: the built-in ones first, then the user's.
pub fn environment_tags() -> Vec<EnvironmentTag> {
    let mut tags: Vec<EnvironmentTag> = EnvironmentType::BUILT_IN
        .into_iter()
        .map(built_in_tag)
        .collect();
    if let Ok(custom) = CUSTOM_TAGS.read() {
        tags.extend(custom.iter().cloned());
    }
    tags
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

    /// Writes on this connection are confirmed first (PROD and any tag set
    /// up that way).
    pub fn confirms_writes(&self) -> bool {
        self.environment_type
            .is_some_and(EnvironmentType::confirms_writes)
    }

    /// A connection opened for this session only, such as a SQLite file
    /// opened directly. It has no row in the app database, so tabs on it are
    /// not persisted.
    pub fn is_temporary(&self) -> bool {
        self.connection_id < 0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tags_resolve_names_and_write_confirmation() {
        assert!(EnvironmentType::PROD.confirms_writes());
        assert!(!EnvironmentType::STAGING.confirms_writes());
        assert_eq!(EnvironmentType::LOCAL.display_name(), "LOCAL");
        assert_eq!(EnvironmentType::from_i32(0), EnvironmentType::DEV);

        let qa = EnvironmentType::from_i32(EnvironmentType::FIRST_CUSTOM_ID);
        set_custom_environment_tags(vec![EnvironmentTag {
            id: qa,
            name: "QA".to_string(),
            color: "magenta".to_string(),
            confirms_writes: true,
        }]);
        assert_eq!(qa.display_name(), "QA");
        assert!(qa.confirms_writes());
        assert_eq!(
            environment_tags().len(),
            EnvironmentType::BUILT_IN.len() + 1
        );

        // A tag that was deleted reads as DEV rather than failing.
        let gone = EnvironmentType::from_i32(EnvironmentType::FIRST_CUSTOM_ID + 7);
        assert_eq!(gone.display_name(), "DEV");
        assert!(!gone.confirms_writes());
    }
}
