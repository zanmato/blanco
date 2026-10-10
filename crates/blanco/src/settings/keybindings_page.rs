use crate::app_settings::AppSettings;
use crate::keybindings::{
    BINDING_GROUPS, BindingDef, CUSTOMIZABLE_BINDINGS, conflicting_actions, default_keystroke,
    effective_keystroke,
};
use crate::settings::Settings;
use gpui::{App, SharedString};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};

/// Build the "Keybindings" settings page. Each customizable action gets a text
/// field for its keystroke (e.g. `cmd-k`, `shift-alt-f`), listed under its
/// group; the page's search finds them by name. Edits are persisted as
/// `keybinding.<action>` settings and take effect on the next launch.
pub fn keybindings_page(
    view_handle: gpui::WeakEntity<super::view::SettingsView>,
    settings: &Settings,
) -> SettingPage {
    let groups = BINDING_GROUPS
        .iter()
        .map(|&group| {
            let items = CUSTOMIZABLE_BINDINGS
                .iter()
                .filter(|def| def.group == group)
                .map(|def| binding_item(def, view_handle.clone(), settings))
                .collect::<Vec<_>>();
            SettingGroup::new().title(group).items(items)
        })
        .collect::<Vec<_>>();

    SettingPage::new("Keybindings")
        .description("Changes take effect after a restart.")
        .resettable(true)
        .groups(groups)
}

fn binding_item(
    def: &'static BindingDef,
    view_handle: gpui::WeakEntity<super::view::SettingsView>,
    settings: &Settings,
) -> SettingItem {
    let action = def.action;
    SettingItem::new(
        def.label,
        SettingField::input(
            move |cx: &App| {
                SharedString::from(effective_keystroke(
                    &AppSettings::global(cx).settings,
                    action,
                ))
            },
            move |val: SharedString, cx: &mut App| {
                let keystroke = val.trim().to_string();
                AppSettings::global_mut(cx)
                    .settings
                    .keybindings
                    .insert(action.to_string(), keystroke.clone());

                if let Some(view) = view_handle.upgrade() {
                    view.update(cx, |view, cx| {
                        view.save_setting_debounced(
                            format!("keybinding.{action}"),
                            keystroke,
                            false,
                            cx,
                        );
                    });
                }
            },
        )
        .default_value(SharedString::from(default_keystroke(action))),
    )
    .description({
        // The page is rebuilt on every render, so a conflict shows as soon as
        // a shortcut is typed.
        let conflicts = conflicting_actions(settings, action);
        let default = default_keystroke(action);
        let mut description = if default.is_empty() {
            "Unbound by default. Enter a keystroke such as `cmd-enter` to assign one.".to_string()
        } else {
            format!("Default: `{default}`. Use chords like `cmd-k` or `shift-alt-f`.")
        };
        if !conflicts.is_empty() {
            description = format!("Conflicts with {}. {description}", conflicts.join(", "));
        }
        description
    })
}
