use crate::app::{
    CommitChanges, DecreaseEditorFontSize, ExplainQuery, FormatQuery, IncreaseEditorFontSize,
    OpenSettings, OpenTerminal, ResetEditorFontSize, RollbackChanges, RunQuery,
    ToggleCommandPalette, ToggleFilterBar,
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
    /// Key context the shortcut is limited to, `None` for the whole window.
    pub context: Option<&'static str>,
}

/// Key context of every text input, the code editors included. The font size
/// shortcuts are limited to it so `ctrl--` and `ctrl-0` still reach the shell
/// while the terminal pane has focus.
const INPUT_CONTEXT: &str = "Input";

/// The set of actions exposed for rebinding. Platform-fixed bindings such as
/// Quit are intentionally excluded.
pub const CUSTOMIZABLE_BINDINGS: &[BindingDef] = &[
    BindingDef {
        action: "RunQuery",
        label: "Run query",
        default: "secondary-enter",
        context: None,
    },
    BindingDef {
        action: "ExplainQuery",
        label: "Explain query",
        default: "secondary-e",
        context: None,
    },
    BindingDef {
        action: "FormatQuery",
        label: "Format SQL",
        default: "shift-alt-f",
        context: None,
    },
    BindingDef {
        action: "ToggleFilterBar",
        label: "Toggle result filter bar",
        default: "secondary-shift-f",
        context: None,
    },
    BindingDef {
        action: "ToggleCommandPalette",
        label: "Toggle command palette",
        default: "secondary-k",
        context: None,
    },
    BindingDef {
        action: "CommitChanges",
        label: "Commit changes",
        default: "super-shift-c",
        context: None,
    },
    BindingDef {
        action: "RollbackChanges",
        label: "Rollback changes",
        default: "super-shift-r",
        context: None,
    },
    BindingDef {
        action: "OpenSettings",
        label: "Open settings",
        default: "super-,",
        context: None,
    },
    BindingDef {
        action: "OpenTerminal",
        label: "Toggle terminal",
        default: "ctrl-`",
        context: None,
    },
    BindingDef {
        action: "IncreaseEditorFontSize",
        label: "Increase editor font size",
        default: "secondary-=",
        context: Some(INPUT_CONTEXT),
    },
    BindingDef {
        action: "DecreaseEditorFontSize",
        label: "Decrease editor font size",
        default: "secondary--",
        context: Some(INPUT_CONTEXT),
    },
    BindingDef {
        action: "ResetEditorFontSize",
        label: "Reset editor font size",
        default: "secondary-0",
        context: Some(INPUT_CONTEXT),
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
fn key_binding(action: &str, keystrokes: &str, context: Option<&str>) -> Option<gpui::KeyBinding> {
    if keystrokes.trim().is_empty() || !keystrokes_valid(keystrokes) {
        return None;
    }

    let binding = match action {
        "RunQuery" => gpui::KeyBinding::new(keystrokes, RunQuery, context),
        "ExplainQuery" => gpui::KeyBinding::new(keystrokes, ExplainQuery, context),
        "FormatQuery" => gpui::KeyBinding::new(keystrokes, FormatQuery, context),
        "ToggleCommandPalette" => gpui::KeyBinding::new(keystrokes, ToggleCommandPalette, context),
        "ToggleFilterBar" => gpui::KeyBinding::new(keystrokes, ToggleFilterBar, context),
        "CommitChanges" => gpui::KeyBinding::new(keystrokes, CommitChanges, context),
        "RollbackChanges" => gpui::KeyBinding::new(keystrokes, RollbackChanges, context),
        "OpenSettings" => gpui::KeyBinding::new(keystrokes, OpenSettings, context),
        "OpenTerminal" => gpui::KeyBinding::new(keystrokes, OpenTerminal, context),
        "IncreaseEditorFontSize" => {
            gpui::KeyBinding::new(keystrokes, IncreaseEditorFontSize, context)
        }
        "DecreaseEditorFontSize" => {
            gpui::KeyBinding::new(keystrokes, DecreaseEditorFontSize, context)
        }
        "ResetEditorFontSize" => gpui::KeyBinding::new(keystrokes, ResetEditorFontSize, context),
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
            key_binding(def.action, &keystrokes, def.context)
        })
        .chain(increase_font_size_alias(settings))
        .collect()
}

/// `secondary-+` as a second shortcut for increasing the font size. On a US
/// layout `+` needs shift, so the default binds the unshifted `=`, but layouts
/// with a dedicated `+` key (Nordic, German) have no `=` to press. Follows the
/// primary shortcut: rebinding or unbinding that one drops the alias.
fn increase_font_size_alias(settings: &Settings) -> Option<gpui::KeyBinding> {
    let action = "IncreaseEditorFontSize";
    (effective_keystroke(settings, action) == default_keystroke(action))
        .then(|| key_binding(action, "secondary-+", Some(INPUT_CONTEXT)))
        .flatten()
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
    fn run_query_is_bound_by_default() {
        let settings = Settings::default();
        assert_eq!(
            effective_keystroke(&settings, "RunQuery"),
            "secondary-enter"
        );
        assert_eq!(effective_keystroke(&settings, "Unknown"), "");
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
        // Unbind RunQuery explicitly and give ExplainQuery a malformed
        // keystroke; both must be skipped rather than panic.
        settings
            .keybindings
            .insert("RunQuery".to_string(), String::new());
        settings
            .keybindings
            .insert("ExplainQuery".to_string(), "ctrl-a-b".to_string());

        let bindings = customizable_key_bindings(&settings);
        // The font size alias adds one binding on top of the table.
        assert_eq!(bindings.len(), CUSTOMIZABLE_BINDINGS.len() + 1 - 2);
    }
}
