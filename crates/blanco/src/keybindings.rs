use crate::app::{
    ActivateNextTab, ActivatePreviousTab, CancelQuery, CloseActiveTab, CommitChanges,
    DecreaseEditorFontSize, ExplainQuery, ExportTableData, FormatQuery, ImportTableData,
    IncreaseEditorFontSize, NewQueryTab, OpenSettings, OpenTerminal, RefreshConnections,
    ResetEditorFontSize, RollbackChanges, RunQuery, ToggleCommandPalette, ToggleFilterBar,
    ToggleResults, ToggleSidebar,
};
use crate::settings::Settings;

/// A keyboard shortcut the user can rebind from the settings UI.
pub struct BindingDef {
    /// Stable identifier persisted in settings (`keybinding.<action>`) and used
    /// to resolve the concrete action when registering the keymap.
    pub action: &'static str,
    /// Human-readable label shown in the settings UI.
    pub label: &'static str,
    /// Heading the settings page lists the action under.
    pub group: &'static str,
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

/// Everywhere but the terminal pane. On Linux and Windows `secondary` is
/// ctrl, and ctrl-w, ctrl-t and ctrl-b mean something to the shell.
const NOT_TERMINAL: &str = "!Terminal";

/// The groups of the settings page, in order.
pub const BINDING_GROUPS: &[&str] = &["Query", "Tabs", "View", "Data", "Editor"];

/// The set of actions exposed for rebinding. Platform-fixed bindings such as
/// Quit are intentionally excluded.
pub const CUSTOMIZABLE_BINDINGS: &[BindingDef] = &[
    BindingDef {
        action: "RunQuery",
        label: "Run current statement",
        group: "Query",
        default: "secondary-enter",
        context: None,
    },
    BindingDef {
        action: "ExplainQuery",
        label: "Explain query",
        group: "Query",
        default: "secondary-e",
        context: None,
    },
    BindingDef {
        action: "CancelQuery",
        label: "Cancel running query",
        group: "Query",
        default: "secondary-.",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "FormatQuery",
        label: "Format SQL",
        group: "Query",
        default: "shift-alt-f",
        context: None,
    },
    BindingDef {
        action: "CommitChanges",
        label: "Commit changes",
        group: "Data",
        default: "super-shift-c",
        context: None,
    },
    BindingDef {
        action: "RollbackChanges",
        label: "Rollback changes",
        group: "Data",
        default: "super-shift-r",
        context: None,
    },
    BindingDef {
        action: "ImportTableData",
        label: "Import into the selected table",
        group: "Data",
        default: "",
        context: None,
    },
    BindingDef {
        action: "ExportTableData",
        label: "Export the selected table",
        group: "Data",
        default: "",
        context: None,
    },
    BindingDef {
        action: "RefreshConnections",
        label: "Refresh connection",
        group: "Data",
        default: "",
        context: None,
    },
    BindingDef {
        action: "NewQueryTab",
        label: "New query tab",
        group: "Tabs",
        default: "secondary-t",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CloseActiveTab",
        label: "Close tab",
        group: "Tabs",
        default: "secondary-w",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "ActivateNextTab",
        label: "Next tab",
        group: "Tabs",
        default: "ctrl-tab",
        context: None,
    },
    BindingDef {
        action: "ActivatePreviousTab",
        label: "Previous tab",
        group: "Tabs",
        default: "ctrl-shift-tab",
        context: None,
    },
    BindingDef {
        action: "ToggleCommandPalette",
        label: "Toggle command palette",
        group: "View",
        default: "secondary-k",
        context: None,
    },
    BindingDef {
        action: "ToggleSidebar",
        label: "Toggle sidebar",
        group: "View",
        default: "secondary-b",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "ToggleResults",
        label: "Toggle results pane",
        group: "View",
        default: "",
        context: None,
    },
    BindingDef {
        action: "ToggleFilterBar",
        label: "Toggle result filter bar",
        group: "View",
        default: "secondary-shift-f",
        context: None,
    },
    BindingDef {
        action: "OpenSettings",
        label: "Open settings",
        group: "View",
        default: "super-,",
        context: None,
    },
    BindingDef {
        action: "OpenTerminal",
        label: "Toggle terminal",
        group: "View",
        default: "ctrl-`",
        context: None,
    },
    BindingDef {
        action: "IncreaseEditorFontSize",
        label: "Increase editor font size",
        group: "Editor",
        default: "secondary-=",
        context: Some(INPUT_CONTEXT),
    },
    BindingDef {
        action: "DecreaseEditorFontSize",
        label: "Decrease editor font size",
        group: "Editor",
        default: "secondary--",
        context: Some(INPUT_CONTEXT),
    },
    BindingDef {
        action: "ResetEditorFontSize",
        label: "Reset editor font size",
        group: "Editor",
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

/// The chords of a shortcut, or `None` when one of them cannot be bound. An
/// empty string has no chords and means "unbound".
pub fn parse_keystrokes(keystrokes: &str) -> Option<Vec<gpui::Keystroke>> {
    keystrokes
        .split_whitespace()
        .map(|chord| {
            gpui::Keystroke::parse(chord)
                .ok()
                .filter(|keystroke| is_bindable(keystroke))
        })
        .collect()
}

/// Whether a keystroke can serve as a shortcut. `Keystroke::parse` takes any
/// word for a key, so the key has to be one a keyboard has, and it needs a
/// modifier other than shift unless it is a function key: a bare letter as a
/// shortcut would swallow typing.
pub fn is_bindable(keystroke: &gpui::Keystroke) -> bool {
    let key = keystroke.key.as_str();
    let is_function_key = key
        .strip_prefix('f')
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()));
    let is_named_key = matches!(
        key,
        "enter"
            | "tab"
            | "space"
            | "escape"
            | "backspace"
            | "delete"
            | "insert"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "up"
            | "down"
            | "left"
            | "right"
    );
    let modifiers = keystroke.modifiers;
    let has_modifier =
        modifiers.control || modifiers.alt || modifiers.platform || modifiers.function;
    is_function_key || (has_modifier && (is_named_key || key.chars().count() == 1))
}

/// True when every space-separated chord can be bound. An empty string is
/// valid and means "unbound".
pub fn keystrokes_valid(keystrokes: &str) -> bool {
    parse_keystrokes(keystrokes).is_some()
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
        "CancelQuery" => gpui::KeyBinding::new(keystrokes, CancelQuery, context),
        "ImportTableData" => gpui::KeyBinding::new(keystrokes, ImportTableData, context),
        "ExportTableData" => gpui::KeyBinding::new(keystrokes, ExportTableData, context),
        "RefreshConnections" => gpui::KeyBinding::new(keystrokes, RefreshConnections, context),
        "NewQueryTab" => gpui::KeyBinding::new(keystrokes, NewQueryTab, context),
        "CloseActiveTab" => gpui::KeyBinding::new(keystrokes, CloseActiveTab, context),
        "ActivateNextTab" => gpui::KeyBinding::new(keystrokes, ActivateNextTab, context),
        "ActivatePreviousTab" => gpui::KeyBinding::new(keystrokes, ActivatePreviousTab, context),
        "ToggleSidebar" => gpui::KeyBinding::new(keystrokes, ToggleSidebar, context),
        "ToggleResults" => gpui::KeyBinding::new(keystrokes, ToggleResults, context),
        _ => return None,
    };
    Some(binding)
}

/// The actions whose shortcut is the same keystroke as `action`'s. Shortcuts
/// limited to different key contexts can still meet (an input sits inside the
/// window), so any two with the same keystroke count as a conflict.
pub fn conflicting_actions(settings: &Settings, action: &str) -> Vec<&'static str> {
    let keystroke = normalized(&effective_keystroke(settings, action));
    if keystroke.is_empty() {
        return Vec::new();
    }
    CUSTOMIZABLE_BINDINGS
        .iter()
        .filter(|other| other.action != action)
        .filter(|other| normalized(&effective_keystroke(settings, other.action)) == keystroke)
        .map(|other| other.label)
        .collect()
}

/// Whether two shortcuts are the same keys, however they are spelled.
pub fn same_keystrokes(a: &str, b: &str) -> bool {
    normalized(a) == normalized(b)
}

/// One spelling per shortcut, so `Secondary-Enter` and a recorded `ctrl-enter`
/// compare equal where secondary is ctrl.
fn normalized(keystrokes: &str) -> String {
    match parse_keystrokes(keystrokes) {
        Some(chords) => chords
            .iter()
            .map(gpui::Keystroke::unparse)
            .collect::<Vec<_>>()
            .join(" "),
        None => keystrokes
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" "),
    }
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
    fn every_binding_resolves_to_an_action() {
        for def in CUSTOMIZABLE_BINDINGS {
            assert!(
                key_binding(def.action, "ctrl-f12", def.context).is_some(),
                "{} has no action",
                def.action
            );
            assert!(BINDING_GROUPS.contains(&def.group), "{}", def.action);
        }
    }

    #[test]
    fn defaults_do_not_conflict_and_overrides_are_flagged() {
        let mut settings = Settings::default();
        for def in CUSTOMIZABLE_BINDINGS {
            assert!(
                conflicting_actions(&settings, def.action).is_empty(),
                "{} conflicts by default",
                def.action
            );
        }
        settings
            .keybindings
            .insert("ToggleResults".to_string(), "Secondary-Enter".to_string());
        assert_eq!(
            conflicting_actions(&settings, "RunQuery"),
            ["Toggle results pane"]
        );
    }

    #[test]
    fn only_real_keystrokes_can_be_bound() {
        for def in CUSTOMIZABLE_BINDINGS {
            assert!(keystrokes_valid(def.default), "{}", def.action);
        }
        for valid in [
            "",
            "f5",
            "ctrl-k",
            "shift-alt-f",
            "super-,",
            "ctrl-k ctrl-s",
        ] {
            assert!(keystrokes_valid(valid), "{valid}");
        }
        for invalid in [
            "ASDASDA",
            "ctrl-asdasda",
            "a",
            "shift-a",
            "enter",
            "ctrl-k x",
        ] {
            assert!(!keystrokes_valid(invalid), "{invalid}");
        }
    }

    #[test]
    fn shortcuts_compare_by_keys_not_spelling() {
        assert!(same_keystrokes("Secondary-Enter", "secondary-enter"));
        assert!(same_keystrokes("shift-alt-f", "alt-shift-f"));
        assert!(!same_keystrokes("ctrl-k", "ctrl-j"));
    }

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
        // The font size alias adds one binding on top of the ones bound by
        // default.
        let bound_by_default = CUSTOMIZABLE_BINDINGS
            .iter()
            .filter(|def| !def.default.is_empty())
            .count();
        assert_eq!(bindings.len(), bound_by_default + 1 - 2);
    }
}
