use crate::app_settings::AppSettings;
use crate::keybindings::{CUSTOMIZABLE_BINDINGS, default_keystroke, effective_keystroke};
use gpui::{App, SharedString};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};

/// Build the "Keybindings" settings page. Each customizable action gets a text
/// field for its keystroke (e.g. `cmd-k`, `shift-alt-f`). Edits are persisted
/// as `keybinding.<action>` settings and take effect on the next launch.
pub fn keybindings_page(view_handle: gpui::WeakEntity<super::view::SettingsView>) -> SettingPage {
    let items = CUSTOMIZABLE_BINDINGS
        .iter()
        .map(|def| {
            let action = def.action;
            let item = SettingItem::new(
                def.label,
                SettingField::input(
                    move |cx: &App| {
                        SharedString::from(effective_keystroke(
                            &AppSettings::global(cx).settings,
                            action,
                        ))
                    },
                    {
                        let view_handle = view_handle.clone();
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
                        }
                    },
                )
                .default_value(SharedString::from(default_keystroke(action))),
            );

            let default = default_keystroke(action);
            let description = if default.is_empty() {
                "Unbound by default. Enter a keystroke such as `cmd-enter` to assign one."
                    .to_string()
            } else {
                format!("Default: `{default}`. Use chords like `cmd-k` or `shift-alt-f`.")
            };

            item.description(description)
        })
        .collect::<Vec<_>>();

    SettingPage::new("Keybindings")
        .resettable(true)
        .groups(vec![
            SettingGroup::new()
                .title("Shortcuts — changes take effect after restart")
                .items(items),
        ])
}
