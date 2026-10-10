//! The filter syntax for object names: `^` anchors the start, `$` the end,
//! and `|` separates alternatives. Matching ignores case.

/// One alternative of a [`NameFilter`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Alternative {
    text: String,
    anchored_start: bool,
    anchored_end: bool,
}

impl Alternative {
    fn matches(&self, name: &str) -> bool {
        match (self.anchored_start, self.anchored_end) {
            (true, true) => name == self.text,
            (true, false) => name.starts_with(&self.text),
            (false, true) => name.ends_with(&self.text),
            (false, false) => name.contains(&self.text),
        }
    }
}

/// A parsed filter. A name matches when any alternative does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NameFilter {
    alternatives: Vec<Alternative>,
}

impl NameFilter {
    /// Parse `pattern`. Empty alternatives are dropped, so `orders|` is the
    /// same as `orders`, while a bare `^` or `$` still anchors an empty text
    /// and matches every name.
    pub fn parse(pattern: &str) -> Self {
        let alternatives = pattern
            .split('|')
            .map(str::trim)
            .filter(|alternative| !alternative.is_empty())
            .map(|alternative| {
                let (anchored_start, rest) = match alternative.strip_prefix('^') {
                    Some(rest) => (true, rest),
                    None => (false, alternative),
                };
                let (anchored_end, text) = match rest.strip_suffix('$') {
                    Some(text) => (true, text),
                    None => (false, rest),
                };
                Alternative {
                    text: text.to_lowercase(),
                    anchored_start,
                    anchored_end,
                }
            })
            .collect();
        Self { alternatives }
    }

    /// True when the filter has nothing to match against.
    pub fn is_empty(&self) -> bool {
        self.alternatives.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.alternatives
            .iter()
            .any(|alternative| alternative.matches(&name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matching<'a>(pattern: &str, names: &[&'a str]) -> Vec<&'a str> {
        let filter = NameFilter::parse(pattern);
        names
            .iter()
            .copied()
            .filter(|name| filter.matches(name))
            .collect()
    }

    #[test]
    fn anchors_and_alternatives() {
        let names = ["users", "user_roles", "app_users", "Orders", "order_items"];
        assert_eq!(
            matching("user", &names),
            ["users", "user_roles", "app_users"]
        );
        assert_eq!(matching("^user", &names), ["users", "user_roles"]);
        assert_eq!(matching("users$", &names), ["users", "app_users"]);
        assert_eq!(matching("^users$", &names), ["users"]);
        assert_eq!(
            matching("^ORDER | items$", &names),
            ["Orders", "order_items"]
        );
        assert_eq!(matching("^users$|^orders$", &names), ["users", "Orders"]);
    }

    #[test]
    fn empty_alternatives_are_ignored() {
        assert!(NameFilter::parse("  ").is_empty());
        assert!(NameFilter::parse("|").is_empty());
        assert_eq!(NameFilter::parse("users|"), NameFilter::parse("users"));
        assert!(NameFilter::parse("^").matches("anything"));
    }
}
