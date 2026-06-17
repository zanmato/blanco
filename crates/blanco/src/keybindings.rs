use crate::app::{
    CommitChanges, ExplainQuery, FormatQuery, OpenSettings, RollbackChanges, RunQuery,
    ToggleCommandPalette,
};
use crate::settings::Settings;

/// A keyboard shortcut the user can rebind from the settings UI.
pub struct BindingDef {
    /// Stable identifier persisted in settings (`keybinding.<action>`) and used
    /// to resolve the concrete action when registering the keymap.
    pub action: &'static str,
    /// Human-readable label shown in the settings UI.
    pub label: &'static str,
    /// Built-in keystroke. An empty string means the action is unbound by
    /// default and only becomes active once the user assigns a shortcut.
    pub default: &'static str,
}

/// The set of actions exposed for rebinding. Platform-fixed bindings such as
/// Quit are intentionally excluded.
pub const CUSTOMIZABLE_BINDINGS: &[BindingDef] = &[
    BindingDef {
        action: "RunQuery",
        label: "Run query",
        default: "",
    },
    BindingDef {
        action: "ExplainQuery",
        label: "Explain query",
        default: "",
    },
    BindingDef {
        action: "FormatQuery",
        label: "Format SQL",
        default: "shift-alt-f",
    },
    BindingDef {
        action: "ToggleCommandPalette",
        label: "Toggle command palette",
        default: "secondary-k",
    },
    BindingDef {
        action: "CommitChanges",
        label: "Commit changes",
        default: "super-shift-c",
    },
    BindingDef {
        action: "RollbackChanges",
        label: "Rollback changes",
        default: "super-shift-r",
    },
    BindingDef {
        action: "OpenSettings",
        label: "Open settings",
        default: "super-,",
    },
];

/// The built-in keystroke for an action, or `""` when the action is unknown.
pub fn default_keystroke(action: &str) -> &'static str {
    CUSTOMIZABLE_BINDINGS
        .iter()
        .find(|binding| binding.action == action)
        .map(|binding| binding.default)
        .unwrap_or("")
}

/// The keystroke currently in effect for an action: the user override if one is
/// set, otherwise the built-in default.
pub fn effective_keystroke(settings: &Settings, action: &str) -> String {
    settings
        .keybindings
        .get(action)
        .cloned()
        .unwrap_or_else(|| default_keystroke(action).to_string())
}

/// True when every space-separated chord parses. An empty string is valid and
/// means "unbound".
pub fn keystrokes_valid(keystrokes: &str) -> bool {
    keystrokes
        .split_whitespace()
        .all(|chord| gpui::Keystroke::parse(chord).is_ok())
}

/// Resolve an action identifier and keystroke into a concrete `KeyBinding`.
/// Returns `None` for empty/invalid keystrokes or unknown actions, so callers
/// never hit `KeyBinding::new`'s internal panic on malformed input.
fn key_binding(action: &str, keystrokes: &str) -> Option<gpui::KeyBinding> {
    if keystrokes.trim().is_empty() || !keystrokes_valid(keystrokes) {
        return None;
    }

    let binding = match action {
        "RunQuery" => gpui::KeyBinding::new(keystrokes, RunQuery, None),
        "ExplainQuery" => gpui::KeyBinding::new(keystrokes, ExplainQuery, None),
        "FormatQuery" => gpui::KeyBinding::new(keystrokes, FormatQuery, None),
        "ToggleCommandPalette" => gpui::KeyBinding::new(keystrokes, ToggleCommandPalette, None),
        "CommitChanges" => gpui::KeyBinding::new(keystrokes, CommitChanges, None),
        "RollbackChanges" => gpui::KeyBinding::new(keystrokes, RollbackChanges, None),
        "OpenSettings" => gpui::KeyBinding::new(keystrokes, OpenSettings, None),
        _ => return None,
    };
    Some(binding)
}

/// Build the customizable key bindings from settings, layering user overrides
/// on top of the built-in defaults. Empty or invalid entries are skipped.
pub fn customizable_key_bindings(settings: &Settings) -> Vec<gpui::KeyBinding> {
    CUSTOMIZABLE_BINDINGS
        .iter()
        .filter_map(|def| {
            let keystrokes = effective_keystroke(settings, def.action);
            key_binding(def.action, &keystrokes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn user_override_replaces_default() {
        let mut settings = Settings::default();
        assert_eq!(effective_keystroke(&settings, "FormatQuery"), "shift-alt-f");

        settings
            .keybindings
            .insert("FormatQuery".to_string(), "cmd-shift-f".to_string());
        assert_eq!(effective_keystroke(&settings, "FormatQuery"), "cmd-shift-f");
    }

    #[test]
    fn unbound_default_is_empty() {
        let settings = Settings::default();
        assert_eq!(effective_keystroke(&settings, "RunQuery"), "");
    }

    #[test]
    fn keystroke_validation_rejects_garbage() {
        assert!(keystrokes_valid("cmd-k"));
        assert!(keystrokes_valid("shift-alt-f"));
        assert!(keystrokes_valid(""));
        // A chord with two non-modifier keys is malformed and would otherwise
        // panic inside `KeyBinding::new`.
        assert!(!keystrokes_valid("ctrl-a-b"));
    }

    #[test]
    fn empty_and_invalid_bindings_are_skipped() {
        let mut settings = Settings::default();
        // RunQuery is unbound by default, so it should not produce a binding.
        // Give ExplainQuery a malformed keystroke; it should be skipped too.
        settings
            .keybindings
            .insert("ExplainQuery".to_string(), "ctrl-a-b".to_string());

        let bindings = customizable_key_bindings(&settings);
        // The five actions with valid defaults still bind.
        assert_eq!(bindings.len(), 5);
    }
}
