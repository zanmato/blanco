/// Table alias information
#[derive(Debug, Clone)]
pub struct TableAlias {
    pub table_name: String,
    pub alias: String,
}

/// SQL parsing context
#[derive(Debug, Clone)]
pub struct SqlContext {
    /// Current word being typed
    pub current_word: String,
    /// Last SQL keyword found
    pub last_keyword: Option<String>,
    /// Table aliases found in query
    pub table_aliases: Vec<TableAlias>,
    /// Whether we're in dot notation context (table.column)
    pub is_dot_notation: bool,
    /// Table name for dot notation (if found)
    pub dot_table_name: Option<String>,
}

/// SQL context parser for extracting table aliases and keywords
pub struct SqlContextParser;

impl SqlContextParser {
    /// Extract current word being typed (for partial matching)
    pub fn extract_current_word(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut end = chars.len();

        // Move backwards while we have valid identifier characters
        while end > 0 {
            let ch = chars[end - 1];
            if ch.is_alphanumeric() || ch == '_' {
                end -= 1;
            } else {
                break;
            }
        }

        text[end..].to_string()
    }

    /// Find the last SQL keyword (case-insensitive, simple lookbehind)
    pub fn find_last_keyword(&self, text: &str) -> Option<String> {
        // Sort keywords by length (longest first) to prioritize multi-word keywords
        let keywords = [
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
        for keyword in &keywords {
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

    /// Extract table aliases from SQL (e.g., "users u", "orders o", "users AS u")
    pub fn extract_table_aliases(&self, text: &str) -> Vec<TableAlias> {
        let mut aliases = Vec::new();
        let words: Vec<&str> = text.split_whitespace().collect();

        let mut i = 0;
        while i < words.len() {
            let word_upper = words[i].to_uppercase();

            // Look for patterns like: table_name alias or table_name AS alias
            if word_upper == "FROM"
                || word_upper == "JOIN"
                || word_upper == "UPDATE"
                || word_upper == "INNER"
                || word_upper == "LEFT"
                || word_upper == "RIGHT"
            {
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
                    if [
                        "WHERE", "ON", "SET", "VALUES", "ORDER", "GROUP", "HAVING", "LIMIT",
                        "UNION",
                    ]
                    .contains(&table_name_upper.as_str())
                    {
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
                            } else if ![
                                "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "ON", "SET", "VALUES",
                                "ORDER", "GROUP", "HAVING", "LIMIT", "UNION", "AS",
                            ]
                            .contains(&next_word_upper.as_str())
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

    /// Parse dot notation context using proper lookbehind logic
    /// Returns: (is_dot_notation, table_name_before_dot, partial_word_after_dot)
    pub fn parse_dot_notation_context(&self, text: &str) -> (bool, Option<String>, String) {
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
                if let Some(table_name) = self.extract_identifier_before_dot(before_dot) {
                    return (true, Some(table_name), current_word);
                }
            }
        }

        // Not dot notation
        (false, None, Self::extract_current_word(text))
    }

    /// Extract identifier (table/alias name) before a dot using lookbehind
    fn extract_identifier_before_dot(&self, text_before_dot: &str) -> Option<String> {
        let chars: Vec<char> = text_before_dot.chars().collect();
        let mut end = chars.len();

        // Skip whitespace before the dot
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }

        // Find the start of the identifier
        let mut start = end;
        while start > 0 {
            let ch = chars[start - 1];
            if ch.is_alphanumeric() || ch == '_' {
                start -= 1;
            } else {
                break;
            }
        }

        // Extract the identifier
        if start < end {
            let identifier: String = chars[start..end].iter().collect();
            if Self::is_valid_identifier(&identifier) {
                return Some(identifier);
            }
        }

        None
    }

    /// Check if a word is a valid identifier
    fn is_valid_identifier(word: &str) -> bool {
        !word.is_empty()
            && word.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !word.chars().next().is_none_or(|c| c.is_ascii_digit())
    }

    /// Resolve table name from alias, returns None if not found
    pub fn resolve_table_alias(&self, aliases: &[TableAlias], alias_name: &str) -> Option<String> {
        for alias_info in aliases {
            if alias_info.alias == alias_name {
                return Some(alias_info.table_name.clone());
            }
        }
        None
    }

    /// Find table name after a specific keyword
    pub fn find_table_after_keyword(&self, text: &str, keyword: &str) -> Option<String> {
        let text_upper = text.to_uppercase();
        if let Some(keyword_pos) = text_upper.rfind(keyword) {
            let after_keyword = &text[keyword_pos + keyword.len()..].trim();
            if let Some(first_word) = after_keyword.split_whitespace().next() {
                let table_name = first_word.trim_end_matches(',').trim_end_matches('(');
                if Self::is_valid_identifier(table_name) && !Self::is_sql_keyword(table_name) {
                    return Some(table_name.to_string());
                }
            }
        }
        None
    }

    /// Find the last table mentioned in the query (for SELECT contexts without explicit table)
    pub fn find_last_table_mentioned(&self, text: &str, aliases: &[TableAlias]) -> Option<String> {
        let text_upper = text.to_uppercase();

        // Look for the last FROM or JOIN clause
        let keywords = ["FROM", "JOIN", "INNER JOIN", "LEFT JOIN", "RIGHT JOIN"];
        let mut last_table = None;
        let mut last_pos = -1;

        for keyword in &keywords {
            if let Some(pos) = text_upper.rfind(keyword)
                && pos as i32 > last_pos
                && let Some(table) = self.find_table_after_keyword(text, keyword)
            {
                last_pos = pos as i32;
                last_table = Some(table);
            }
        }

        // If we found a table, try to resolve it through aliases
        if let Some(table_name) = last_table {
            if let Some(resolved) = self.resolve_table_alias(aliases, &table_name) {
                return Some(resolved);
            }
            return Some(table_name);
        }

        None
    }

    /// Generate table abbreviation from table name
    /// Examples: "products" -> "products p", "localized_products" -> "localized_products lp"
    pub fn generate_table_abbreviation(&self, table_name: &str) -> String {
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

    // Check if a word is a SQL keyword
    fn is_sql_keyword(word: &str) -> bool {
        let sql_keywords = [
            "SELECT", "FROM", "WHERE", "AND", "OR", "ORDER", "GROUP", "HAVING", "BY", "SET",
            "VALUES", "INSERT", "DELETE", "UPDATE", "INTO", "JOIN", "INNER", "LEFT", "RIGHT",
            "OUTER", "ON", "AS", "DISTINCT", "COUNT", "SUM", "AVG", "MAX", "MIN", "NOT", "NULL",
            "IS", "IN", "EXISTS", "BETWEEN", "LIKE",
        ];

        sql_keywords.contains(&word.to_uppercase().as_str())
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
        let parser = SqlContextParser;

        // Test basic alias patterns
        let aliases = parser.extract_table_aliases("FROM users u");
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].table_name, "users");
        assert_eq!(aliases[0].alias, "u");

        // Test AS keyword
        let aliases = parser.extract_table_aliases("FROM users AS u");
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].table_name, "users");
        assert_eq!(aliases[0].alias, "u");

        // Test multiple tables
        let aliases =
            parser.extract_table_aliases("FROM users u JOIN orders o ON u.id = o.user_id");
        assert_eq!(aliases.len(), 2);
        assert_eq!(aliases[0].table_name, "users");
        assert_eq!(aliases[0].alias, "u");
        assert_eq!(aliases[1].table_name, "orders");
        assert_eq!(aliases[1].alias, "o");

        // Test with JOIN keywords
        let aliases = parser.extract_table_aliases("SELECT * FROM users u INNER JOIN orders o");
        assert_eq!(aliases.len(), 2);
    }

    #[test]
    fn test_resolve_table_alias() {
        let parser = SqlContextParser;
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
            parser.resolve_table_alias(&aliases, "u"),
            Some("users".to_string())
        );
        assert_eq!(
            parser.resolve_table_alias(&aliases, "o"),
            Some("orders".to_string())
        );
        assert_eq!(parser.resolve_table_alias(&aliases, "x"), None);
    }

    #[test]
    fn test_generate_table_abbreviation() {
        let parser = SqlContextParser;

        // Test simple table name
        assert_eq!(
            parser.generate_table_abbreviation("products"),
            "products p"
        );

        // Test multi-word table name with underscores
        assert_eq!(
            parser.generate_table_abbreviation("localized_products"),
            "localized_products lp"
        );

        // Test three parts
        assert_eq!(
            parser.generate_table_abbreviation("user_order_items"),
            "user_order_items uoi"
        );

        // Test single character
        assert_eq!(parser.generate_table_abbreviation("a"), "a a");

        // Test empty string (edge case)
        assert_eq!(parser.generate_table_abbreviation(""), " ");
    }
}
