use sql_parser::statement_parser::{TableAlias, ident_eq};

/// Resolve table name from alias, returns None if not found. Matching is
/// case-insensitive (and quote-insensitive) so `FROM Users u ... u.` resolves
/// regardless of how the alias was cased at the reference site.
pub fn resolve_table_alias(aliases: &[TableAlias], alias_name: &str) -> Option<String> {
    for alias_info in aliases {
        if ident_eq(&alias_info.alias, alias_name) {
            return Some(alias_info.table_name.clone());
        }
    }
    None
}

/// Generate table abbreviation from table name
/// Examples: "products" -> "products p", "localized_products" -> "localized_products lp"
pub fn generate_table_abbreviation(table_name: &str) -> String {
    let parts: Vec<&str> = table_name.split('_').collect();
    let abbreviation: String = parts
        .iter()
        .map(|part| part.chars().next().unwrap_or(' '))
        .filter(|c| *c != ' ')
        .collect();

    format!("{} {}", table_name, abbreviation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_table_alias() {
        let aliases = vec![
            TableAlias {
                table_name: "users".to_string(),
                alias: "u".to_string(),
            },
            TableAlias {
                table_name: "orders".to_string(),
                alias: "o".to_string(),
            },
        ];

        assert_eq!(
            resolve_table_alias(&aliases, "u"),
            Some("users".to_string())
        );
        assert_eq!(
            resolve_table_alias(&aliases, "o"),
            Some("orders".to_string())
        );
        assert_eq!(resolve_table_alias(&aliases, "x"), None);
    }

    #[test]
    fn test_generate_table_abbreviation() {
        assert_eq!(generate_table_abbreviation("products"), "products p");
        assert_eq!(
            generate_table_abbreviation("localized_products"),
            "localized_products lp"
        );
        assert_eq!(
            generate_table_abbreviation("user_order_items"),
            "user_order_items uoi"
        );
        assert_eq!(generate_table_abbreviation("a"), "a a");
        assert_eq!(generate_table_abbreviation(""), " ");
    }
}
