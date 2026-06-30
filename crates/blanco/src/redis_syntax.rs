use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};

/// Register the Redis grammar for the code editor's syntax highlighting.
///
/// Mirrors [`crate::sql::register_languages`]: we register our vendored
/// `tree-sitter-redis` grammar under the `"redis"` language name so editor tabs
/// backed by a Redis connection get command/argument highlighting through the
/// same gpui-component pipeline as SQL tabs.
pub fn register_language() {
    LanguageRegistry::singleton().register(
        "redis",
        &LanguageConfig::new(
            "redis",
            tree_sitter_redis::LANGUAGE.into(),
            vec![],
            tree_sitter_redis::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
}
