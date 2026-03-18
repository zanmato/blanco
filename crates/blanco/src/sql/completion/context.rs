/// Table alias information
#[derive(Debug, Clone)]
pub struct TableAlias {
    pub table_name: String,
    pub alias: String,
}

/// Complete parsed SQL metadata from a single parse operation
#[derive(Debug, Clone)]
pub struct ParsedSqlContext {
    pub current_word: String,
    pub last_keyword: Option<String>,
    pub table_aliases: Vec<TableAlias>,
    pub is_dot_notation: bool,
    pub dot_table_name: Option<String>,
    /// The original text that was parsed
    pub text: String,
}

/// SQL context parser for extracting table aliases and keywords
pub struct SqlContextParser;

impl SqlContextParser {
    /// Parse SQL text once and return all metadata in a single pass
    /// This is more efficient than calling individual parsing methods separately
    pub fn parse(text: &str) -> ParsedSqlContext {
        let current_word = Self::extract_current_word(text);
        let last_keyword = Self::find_last_keyword(text);
        let table_aliases = Self::extract_table_aliases(text);
        let (is_dot_notation, dot_table_name, _) = Self::parse_dot_notation(text);

        ParsedSqlContext {
            current_word,
            last_keyword,
            table_aliases,
            is_dot_notation,
            dot_table_name,
            text: text.to_string(),
        }
    }

    /// Extract current word being typed (for partial matching)
    pub fn extract_current_word(text: &str) -> String {
        for (byte_offset, ch) in text.char_indices().rev() {
            if !(ch.is_alphanumeric() || ch == '_') {
                let start = byte_offset + ch.len_utf8();
                return text[start..].to_string();
            }
        }
        text.to_string()
    }

    // === Internal parsing methods ===

    pub fn find_last_keyword(text: &str) -> Option<String> {
        // Sort keywords by length (longest first) to prioritize multi-word keywords
        const KEYWORDS: &[&str] = &[
            "INNER JOIN",
            "LEFT JOIN",
            "RIGHT JOIN",
            "OUTER JOIN",
            "ORDER BY",
            "GROUP BY",
            "SELECT",
            "FROM",
            "WHERE",
            "JOIN",
            "UPDATE",
            "INSERT",
            "INTO",
            "DELETE",
            "SET",
            "VALUES",
            "HAVING",
            "LIMIT",
            "ON",
            "AND",
            "OR",
            "NOT",
        ];

        let text_upper = text.to_uppercase();
        let mut last_keyword_pos = -1;
        let mut last_keyword = None;

        // Simple approach: find the last occurrence of each keyword
        for keyword in KEYWORDS {
            if let Some(pos) = text_upper.rfind(keyword) {
                // Simple boundary check - ensure it's not part of a larger word
                let is_word_boundary = (pos == 0
                    || !text.chars().nth(pos - 1).unwrap_or(' ').is_alphanumeric())
                    && (pos + keyword.len() >= text.len()
                        || !text
                            .chars()
                            .nth(pos + keyword.len())
                            .unwrap_or(' ')
                            .is_alphanumeric());

                if is_word_boundary && pos as i32 > last_keyword_pos {
                    last_keyword_pos = pos as i32;
                    last_keyword = Some(keyword.to_string());
                }
            }
        }

        last_keyword
    }

    pub fn extract_table_aliases(text: &str) -> Vec<TableAlias> {
        const TABLE_KEYWORDS: &[&str] = &["FROM", "JOIN", "UPDATE", "INNER", "LEFT", "RIGHT"];
        const STOP_KEYWORDS: &[&str] = &[
            "WHERE", "ON", "SET", "VALUES", "ORDER", "GROUP", "HAVING", "LIMIT", "UNION",
        ];
        const ALIAS_STOP_KEYWORDS: &[&str] = &[
            "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "ON", "SET", "VALUES", "ORDER",
            "GROUP", "HAVING", "LIMIT", "UNION", "AS",
        ];

        let mut aliases = Vec::new();
        let words: Vec<&str> = text.split_whitespace().collect();

        let mut i = 0;
        while i < words.len() {
            let word_upper = words[i].to_uppercase();

            // Look for patterns like: table_name alias or table_name AS alias
            if TABLE_KEYWORDS.contains(&word_upper.as_str()) {
                let mut table_name_idx = i + 1;

                // Skip JOIN keywords to get to table name
                if (word_upper == "INNER" || word_upper == "LEFT" || word_upper == "RIGHT")
                    && table_name_idx < words.len()
                    && words[table_name_idx].to_uppercase() == "JOIN"
                {
                    table_name_idx += 1;
                }

                // Extract table name and alias
                if table_name_idx < words.len() {
                    let table_name = words[table_name_idx];

                    // Stop if we hit a keyword that indicates end of table reference
                    let table_name_upper = table_name.to_uppercase();
                    if STOP_KEYWORDS.contains(&table_name_upper.as_str()) {
                        // Skip this word, it's not a table name
                    } else {
                        // Check for AS alias or direct alias
                        if table_name_idx + 1 < words.len() {
                            let next_word_upper = words[table_name_idx + 1].to_uppercase();
                            if next_word_upper == "AS" && table_name_idx + 2 < words.len() {
                                // table_name AS alias
                                let alias = words[table_name_idx + 2];
                                aliases.push(TableAlias {
                                    table_name: table_name.trim_end_matches(';').to_string(),
                                    alias: alias.trim_end_matches(';').to_string(),
                                });
                                i = table_name_idx + 2;
                            } else if !ALIAS_STOP_KEYWORDS.contains(&next_word_upper.as_str())
                            {
                                // table_name alias
                                let alias = words[table_name_idx + 1];
                                aliases.push(TableAlias {
                                    table_name: table_name.trim_end_matches(';').to_string(),
                                    alias: alias.trim_end_matches(';').to_string(),
                                });
                                i = table_name_idx + 1;
                            }
                            // If we reach here, we have just table_name without alias
                        }
                        // If we reach here, we have just table_name at end of query
                    }
                }
            }
            i += 1;
        }

        aliases
    }

    pub fn parse_dot_notation(text: &str) -> (bool, Option<String>, String) {
        // Look for the last dot in the text
        if let Some(dot_pos) = text.rfind('.') {
            let before_dot = &text[..dot_pos];
            let after_dot = &text[dot_pos + 1..];

            // Extract the partial word after the dot (first word after the dot)
            let partial_word_after_dot = if after_dot.trim().is_empty() {
                "".to_string()
            } else {
                // Get the first word after the dot
                after_dot
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string()
            };

            // Extract the current word at the end of the text
            let current_word = Self::extract_current_word(text);

            // Check if this should be considered dot notation:
            // 1. Text ends with a dot (cursor right after dot)
            // 2. OR the characters immediately after the dot form a word that could be a column name
            if text.ends_with('.') || !partial_word_after_dot.is_empty() {
                // Extract the table/alias name before the dot
                if let Some(table_name) = Self::extract_identifier_before_dot(before_dot) {
                    return (true, Some(table_name), current_word);
                }
            }
        }

        // Not dot notation
        (false, None, Self::extract_current_word(text))
    }

    /// Extract identifier (table/alias name) before a dot using lookbehind
    fn extract_identifier_before_dot(text_before_dot: &str) -> Option<String> {
        // Skip trailing whitespace
        let trimmed_end = text_before_dot.trim_end_matches(' ');
        let trimmed_end = trimmed_end.trim_end_matches('\t');

        // Find the identifier boundary by iterating backwards
        for (byte_offset, ch) in trimmed_end.char_indices().rev() {
            if !(ch.is_alphanumeric() || ch == '_') {
                let start = byte_offset + ch.len_utf8();
                let identifier = &trimmed_end[start..];
                if super::is_valid_identifier(identifier) {
                    return Some(identifier.to_string());
                }
                return None;
            }
        }

        // All characters are valid identifier characters
        if super::is_valid_identifier(trimmed_end) {
            Some(trimmed_end.to_string())
        } else {
            None
        }
    }

    /// Resolve table name from alias, returns None if not found
    pub fn resolve_table_alias(aliases: &[TableAlias], alias_name: &str) -> Option<String> {
        for alias_info in aliases {
            if alias_info.alias == alias_name {
                return Some(alias_info.table_name.clone());
            }
        }
        None
    }

    /// Find table name after a specific keyword
    pub fn find_table_after_keyword(text: &str, keyword: &str) -> Option<String> {
        let text_upper = text.to_uppercase();
        if let Some(keyword_pos) = text_upper.rfind(keyword) {
            let after_keyword = &text[keyword_pos + keyword.len()..].trim();
            if let Some(first_word) = after_keyword.split_whitespace().next() {
                let table_name = first_word.trim_end_matches(',').trim_end_matches('(');
                if super::is_valid_identifier(table_name) && !super::is_sql_keyword(table_name) {
                    return Some(table_name.to_string());
                }
            }
        }
        None
    }

    /// Find the last table mentioned in the query (for SELECT contexts without explicit table)
    pub fn find_last_table_mentioned(text: &str, aliases: &[TableAlias]) -> Option<String> {
        let text_upper = text.to_uppercase();

        // Look for the last FROM or JOIN clause
        const KEYWORDS: &[&str] = &["FROM", "JOIN", "INNER JOIN", "LEFT JOIN", "RIGHT JOIN"];
        let mut last_table = None;
        let mut last_pos = -1;

        for keyword in KEYWORDS {
            if let Some(pos) = text_upper.rfind(keyword)
                && pos as i32 > last_pos
                && let Some(table) = Self::find_table_after_keyword(text, keyword)
            {
                last_pos = pos as i32;
                last_table = Some(table);
            }
        }

        // If we found a table, try to resolve it through aliases
        if let Some(table_name) = last_table {
            if let Some(resolved) = Self::resolve_table_alias(aliases, &table_name) {
                return Some(resolved);
            }
            return Some(table_name);
        }

        None
    }

    /// Generate table abbreviation from table name
    /// Examples: "products" -> "products p", "localized_products" -> "localized_products lp"
    pub fn generate_table_abbreviation(table_name: &str) -> String {
        // Split on underscores and take first letter of each part
        let parts: Vec<&str> = table_name.split('_').collect();
        let abbreviation: String = parts
            .iter()
            .map(|part| {
                // Take first character of each part
                part.chars().next().unwrap_or(' ')
            })
            .filter(|c| *c != ' ')
            .collect();

        format!("{} {}", table_name, abbreviation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_current_word() {
        assert_eq!(SqlContextParser::extract_current_word("SELECT * FROM"), "FROM");
        assert_eq!(SqlContextParser::extract_current_word("SELECT * F"), "F");
        assert_eq!(SqlContextParser::extract_current_word("SELECT * "), "");
        assert_eq!(SqlContextParser::extract_current_word("user_name"), "user_name");
        assert_eq!(SqlContextParser::extract_current_word("123"), "123");
    }

    #[test]
    fn test_extract_table_aliases() {
        // Test basic alias patterns
        let aliases = SqlContextParser::extract_table_aliases("FROM users u");
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].table_name, "users");
        assert_eq!(aliases[0].alias, "u");

        // Test AS keyword
        let aliases = SqlContextParser::extract_table_aliases("FROM users AS u");
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].table_name, "users");
        assert_eq!(aliases[0].alias, "u");

        // Test multiple tables
        let aliases =
            SqlContextParser::extract_table_aliases("FROM users u JOIN orders o ON u.id = o.user_id");
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[0].table_name, "users");
        assert_eq!(aliases[0].alias, "u");
        assert_eq!(aliases[1].table_name, "orders");
        assert_eq!(aliases[1].alias, "o");

        // Test with JOIN keywords
        let aliases = SqlContextParser::extract_table_aliases("SELECT * FROM users u INNER JOIN orders o");
        assert_eq!(aliases.len(), 2);
    }

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
            SqlContextParser::resolve_table_alias(&aliases, "u"),
            Some("users".to_string())
        );
        assert_eq!(
            SqlContextParser::resolve_table_alias(&aliases, "o"),
            Some("orders".to_string())
        );
        assert_eq!(SqlContextParser::resolve_table_alias(&aliases, "x"), None);
    }

    #[test]
    fn test_generate_table_abbreviation() {
        // Test simple table name
        assert_eq!(
            SqlContextParser::generate_table_abbreviation("products"),
            "products p"
        );

        // Test multi-word table name with underscores
        assert_eq!(
            SqlContextParser::generate_table_abbreviation("localized_products"),
            "localized_products lp"
        );

        // Test three parts
        assert_eq!(
            SqlContextParser::generate_table_abbreviation("user_order_items"),
            "user_order_items uoi"
        );

        // Test single character
        assert_eq!(SqlContextParser::generate_table_abbreviation("a"), "a a");

        // Test empty string (edge case)
        assert_eq!(SqlContextParser::generate_table_abbreviation(""), " ");
    }
}
