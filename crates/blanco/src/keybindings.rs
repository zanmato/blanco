use crate::app::{
    ActivateNextTab, ActivatePreviousTab, CancelQuery, CloseActiveTab, CloseAllTabs,
    CloseOtherTabs, CloseTabsToTheLeft, CloseTabsToTheRight, CloseWindow, CommitChanges,
    CopySelection, DecreaseEditorFontSize, ExplainAnalyzeQuery, ExplainQuery, ExportTableData,
    FormatQuery, ImportTableData, IncreaseEditorFontSize, LowerCaseKeywords, NewQueryTab,
    NewWindow, OpenReferencedRow, OpenSettings, OpenTerminal, RefreshConnections, ReopenClosedTab,
    ResetEditorFontSize, RollbackChanges, RunQuery, SaveTab, SaveTabAs, ToggleCommandPalette,
    ToggleFilterBar, ToggleResults, ToggleSidebar, UpperCaseKeywords,
};
use crate::settings::Settings;
use crate::snippets_panel::{DuplicateSnippet, RenameSnippet};
use gpui::SharedString;

/// The OS that resolves `secondary-` in a shortcut. Blanco runs natively, so
/// it is the one the app was built for, as `App::platform_info` reports it.
pub const HOST_OS: gpui::OperatingSystem = if cfg!(target_os = "macos") {
    gpui::OperatingSystem::Mac
} else if cfg!(target_os = "windows") {
    gpui::OperatingSystem::Windows
} else {
    gpui::OperatingSystem::Linux
};

/// A keyboard shortcut the user can rebind from the settings UI.
pub struct BindingDef {
    /// Stable identifier persisted in settings (`keybinding.<action>`) and used
    /// to resolve the concrete action when registering the keymap.
    pub action: &'static str,
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
pub const BINDING_GROUPS: &[&str] = &[
    "Query", "File", "Tabs", "View", "Data", "Editor", "Snippets",
];

/// The set of actions exposed for rebinding. Platform-fixed bindings such as
/// Quit are intentionally excluded.
pub const CUSTOMIZABLE_BINDINGS: &[BindingDef] = &[
    BindingDef {
        action: "RunQuery",
        group: "Query",
        default: "secondary-enter",
        context: None,
    },
    BindingDef {
        action: "ExplainQuery",
        group: "Query",
        default: "secondary-e",
        context: None,
    },
    BindingDef {
        action: "ExplainAnalyzeQuery",
        group: "Query",
        default: "secondary-shift-e",
        context: None,
    },
    BindingDef {
        action: "CancelQuery",
        group: "Query",
        default: "secondary-.",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "FormatQuery",
        group: "Query",
        default: "shift-alt-f",
        context: None,
    },
    BindingDef {
        action: "UpperCaseKeywords",
        group: "Query",
        default: "shift-alt-u",
        context: None,
    },
    BindingDef {
        action: "LowerCaseKeywords",
        group: "Query",
        default: "shift-alt-l",
        context: None,
    },
    BindingDef {
        action: "NewWindow",
        group: "File",
        default: "secondary-shift-n",
        context: None,
    },
    BindingDef {
        action: "CloseWindow",
        group: "File",
        default: "secondary-shift-w",
        context: None,
    },
    BindingDef {
        action: "SaveTab",
        group: "File",
        default: "secondary-s",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "SaveTabAs",
        group: "File",
        default: "secondary-shift-s",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CommitChanges",
        group: "Data",
        default: "super-shift-c",
        context: None,
    },
    BindingDef {
        action: "RollbackChanges",
        group: "Data",
        default: "super-shift-r",
        context: None,
    },
    BindingDef {
        action: "ImportTableData",
        group: "Data",
        default: "",
        context: None,
    },
    BindingDef {
        action: "ExportTableData",
        group: "Data",
        default: "",
        context: None,
    },
    BindingDef {
        action: "RefreshConnections",
        group: "Data",
        default: "",
        context: None,
    },
    BindingDef {
        action: "NewQueryTab",
        group: "Tabs",
        default: "secondary-t",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CloseActiveTab",
        group: "Tabs",
        default: "secondary-w",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CloseOtherTabs",
        group: "Tabs",
        default: "secondary-alt-w",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CloseTabsToTheLeft",
        group: "Tabs",
        default: "",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CloseTabsToTheRight",
        group: "Tabs",
        default: "",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "CloseAllTabs",
        group: "Tabs",
        default: "secondary-alt-shift-w",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "ActivateNextTab",
        group: "Tabs",
        default: "ctrl-tab",
        context: None,
    },
    BindingDef {
        action: "ActivatePreviousTab",
        group: "Tabs",
        default: "ctrl-shift-tab",
        context: None,
    },
    BindingDef {
        action: "ToggleCommandPalette",
        group: "View",
        default: "secondary-k",
        context: None,
    },
    BindingDef {
        action: "ToggleSidebar",
        group: "View",
        default: "secondary-b",
        context: Some(NOT_TERMINAL),
    },
    BindingDef {
        action: "ToggleResults",
        group: "View",
        default: "",
        context: None,
    },
    BindingDef {
        action: "ToggleFilterBar",
        group: "View",
        default: "secondary-shift-f",
        context: None,
    },
    BindingDef {
        action: "OpenSettings",
        group: "View",
        default: "super-,",
        context: None,
    },
    BindingDef {
        action: "OpenTerminal",
        group: "View",
        default: "ctrl-`",
        context: None,
    },
    BindingDef {
        action: "RenameSnippet",
        group: "Snippets",
        default: "f2",
        context: None,
    },
    BindingDef {
        action: "DuplicateSnippet",
        group: "Snippets",
        default: "secondary-d",
        context: Some(blanco_ui::draggable_tree::CONTEXT),
    },
    BindingDef {
        action: "ReopenClosedTab",
        group: "Tabs",
        default: "secondary-shift-t",
        context: None,
    },
    BindingDef {
        action: "CopySelection",
        group: "Data",
        default: "secondary-c",
        context: Some("DataTable"),
    },
    BindingDef {
        action: "OpenReferencedRow",
        group: "Data",
        default: "alt-enter",
        context: Some("DataTable"),
    },
    BindingDef {
        action: "IncreaseEditorFontSize",
        group: "Editor",
        default: "secondary-=",
        context: Some(INPUT_CONTEXT),
    },
    BindingDef {
        action: "DecreaseEditorFontSize",
        group: "Editor",
        default: "secondary--",
        context: Some(INPUT_CONTEXT),
    },
    BindingDef {
        action: "ResetEditorFontSize",
        group: "Editor",
        default: "secondary-0",
        context: Some(INPUT_CONTEXT),
    },
];

/// The translated label of a customizable action, shown on the settings
/// page and in the command palette so a command reads the same in both.
pub fn action_label(action: &str) -> SharedString {
    t!(format!("settings.keybindings.actions.{action}"))
        .into_owned()
        .into()
}

/// The translated heading of a group of `BINDING_GROUPS`.
pub fn group_label(group: &str) -> SharedString {
    t!(format!(
        "settings.keybindings.groups.{}",
        group.to_lowercase()
    ))
    .into_owned()
    .into()
}

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
            gpui::Keystroke::parse_with_os(chord, HOST_OS)
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
/// never hit a parse error on malformed input.
fn key_binding(
    action: &str,
    keystrokes: &str,
    context: Option<&str>,
    cx: &gpui::App,
) -> Option<gpui::KeyBinding> {
    if keystrokes.trim().is_empty() || !keystrokes_valid(keystrokes) {
        return None;
    }

    let action: Box<dyn gpui::Action> = match action {
        "RunQuery" => Box::new(RunQuery),
        "ExplainQuery" => Box::new(ExplainQuery),
        "ExplainAnalyzeQuery" => Box::new(ExplainAnalyzeQuery),
        "FormatQuery" => Box::new(FormatQuery),
        "UpperCaseKeywords" => Box::new(UpperCaseKeywords),
        "LowerCaseKeywords" => Box::new(LowerCaseKeywords),
        "ToggleCommandPalette" => Box::new(ToggleCommandPalette),
        "ToggleFilterBar" => Box::new(ToggleFilterBar),
        "NewWindow" => Box::new(NewWindow),
        "CloseWindow" => Box::new(CloseWindow),
        "SaveTab" => Box::new(SaveTab),
        "SaveTabAs" => Box::new(SaveTabAs),
        "CommitChanges" => Box::new(CommitChanges),
        "RollbackChanges" => Box::new(RollbackChanges),
        "OpenSettings" => Box::new(OpenSettings),
        "OpenTerminal" => Box::new(OpenTerminal),
        "ReopenClosedTab" => Box::new(ReopenClosedTab),
        "OpenReferencedRow" => Box::new(OpenReferencedRow),
        "CopySelection" => Box::new(CopySelection),
        "IncreaseEditorFontSize" => Box::new(IncreaseEditorFontSize),
        "DecreaseEditorFontSize" => Box::new(DecreaseEditorFontSize),
        "ResetEditorFontSize" => Box::new(ResetEditorFontSize),
        "CancelQuery" => Box::new(CancelQuery),
        "ImportTableData" => Box::new(ImportTableData),
        "ExportTableData" => Box::new(ExportTableData),
        "RefreshConnections" => Box::new(RefreshConnections),
        "NewQueryTab" => Box::new(NewQueryTab),
        "CloseActiveTab" => Box::new(CloseActiveTab),
        "RenameSnippet" => Box::new(RenameSnippet),
        "DuplicateSnippet" => Box::new(DuplicateSnippet),
        "CloseOtherTabs" => Box::new(CloseOtherTabs),
        "CloseTabsToTheLeft" => Box::new(CloseTabsToTheLeft),
        "CloseTabsToTheRight" => Box::new(CloseTabsToTheRight),
        "CloseAllTabs" => Box::new(CloseAllTabs),
        "ActivateNextTab" => Box::new(ActivateNextTab),
        "ActivatePreviousTab" => Box::new(ActivatePreviousTab),
        "ToggleSidebar" => Box::new(ToggleSidebar),
        "ToggleResults" => Box::new(ToggleResults),
        _ => return None,
    };
    let context = match context {
        Some(context) => Some(
            gpui::KeyBindingContextPredicate::parse(context)
                .ok()?
                .into(),
        ),
        None => None,
    };
    gpui::KeyBinding::load(keystrokes, action, context, false, None, cx).ok()
}

/// A fixed shortcut that may use `secondary-`, which `KeyBinding::new` no
/// longer accepts. Panics on an invalid shortcut or context, like `new`.
pub fn host_key_binding<A: gpui::Action>(
    keystrokes: &str,
    action: A,
    context: Option<&str>,
    cx: &gpui::App,
) -> gpui::KeyBinding {
    let context = context.map(|context| {
        gpui::KeyBindingContextPredicate::parse(context)
            .expect("valid key context")
            .into()
    });
    gpui::KeyBinding::load(keystrokes, Box::new(action), context, false, None, cx)
        .expect("valid keystrokes")
}

/// The actions (by `BindingDef::action`) whose shortcut is the same keystroke as `action`'s. Shortcuts
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
        .map(|other| other.action)
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
            .map(|keystroke| keystroke.unparse(HOST_OS))
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
pub fn customizable_key_bindings(settings: &Settings, cx: &gpui::App) -> Vec<gpui::KeyBinding> {
    CUSTOMIZABLE_BINDINGS
        .iter()
        .filter_map(|def| {
            let keystrokes = effective_keystroke(settings, def.action);
            key_binding(def.action, &keystrokes, def.context, cx)
        })
        .chain(increase_font_size_alias(settings, cx))
        .collect()
}

/// `secondary-+` as a second shortcut for increasing the font size. On a US
/// layout `+` needs shift, so the default binds the unshifted `=`, but layouts
/// with a dedicated `+` key (Nordic, German) have no `=` to press. Follows the
/// primary shortcut: rebinding or unbinding that one drops the alias.
fn increase_font_size_alias(settings: &Settings, cx: &gpui::App) -> Option<gpui::KeyBinding> {
    let action = "IncreaseEditorFontSize";
    (effective_keystroke(settings, action) == default_keystroke(action))
        .then(|| key_binding(action, "secondary-+", Some(INPUT_CONTEXT), cx))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[gpui::test]
    fn every_binding_resolves_to_an_action(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            for def in CUSTOMIZABLE_BINDINGS {
                assert!(
                    key_binding(def.action, "ctrl-f12", def.context, cx).is_some(),
                    "{} has no action",
                    def.action
                );
                assert!(BINDING_GROUPS.contains(&def.group), "{}", def.action);
            }
        });
    }

    /// Action and group labels are looked up by a key built at run time,
    /// which the locale test cannot see, so a new action needs its key here.
    #[test]
    fn every_action_and_group_has_a_label() {
        let backend = crate::_rust_i18n_backend();
        let keys = CUSTOMIZABLE_BINDINGS
            .iter()
            .map(|def| format!("settings.keybindings.actions.{}", def.action))
            .chain(
                BINDING_GROUPS
                    .iter()
                    .map(|group| format!("settings.keybindings.groups.{}", group.to_lowercase())),
            );
        for key in keys {
            assert!(backend.translate("en", &key).is_some(), "missing {key}");
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
            ["ToggleResults"]
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

    #[gpui::test]
    fn empty_and_invalid_bindings_are_skipped(cx: &mut gpui::TestAppContext) {
        let mut settings = Settings::default();
        // Unbind RunQuery explicitly and give ExplainQuery a malformed
        // keystroke; both must be skipped rather than panic.
        settings
            .keybindings
            .insert("RunQuery".to_string(), String::new());
        settings
            .keybindings
            .insert("ExplainQuery".to_string(), "ctrl-a-b".to_string());

        let bindings = cx.update(|cx| customizable_key_bindings(&settings, cx));
        // The font size alias adds one binding on top of the ones bound by
        // default.
        let bound_by_default = CUSTOMIZABLE_BINDINGS
            .iter()
            .filter(|def| !def.default.is_empty())
            .count();
        assert_eq!(bindings.len(), bound_by_default + 1 - 2);
    }
}
