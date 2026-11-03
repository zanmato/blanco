use crate::app_settings::AppSettings;
use crate::settings::{CsvDelimiter, CsvSettings, Settings};
use gpui::{App, SharedString, WeakEntity};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem};
use transformers::CsvQuoting;

use super::view::SettingsView;

/// Defaults for Copy as CSV, which the export dialog starts from too.
pub fn csv_group(
    view_handle: WeakEntity<SettingsView>,
    default_settings: &Settings,
) -> SettingGroup {
    let defaults = &default_settings.csv;

    // Apply a change to the CSV settings and persist it under `key`.
    let setter = |key: &'static str, apply: fn(&mut CsvSettings, &str)| {
        let view_handle = view_handle.clone();
        move |value: SharedString, cx: &mut App| {
            apply(&mut AppSettings::global_mut(cx).settings.csv, &value);
            if let Some(view) = view_handle.upgrade() {
                view.update(cx, |view, cx| {
                    view.save_setting_debounced(key.into(), value.to_string(), false, cx);
                });
            }
        }
    };
    let switch_setter = |key: &'static str, apply: fn(&mut CsvSettings, bool)| {
        let view_handle = view_handle.clone();
        move |value: bool, cx: &mut App| {
            apply(&mut AppSettings::global_mut(cx).settings.csv, value);
            if let Some(view) = view_handle.upgrade() {
                view.update(cx, |view, cx| {
                    view.save_setting_debounced(key.into(), value.to_string(), false, cx);
                });
            }
        }
    };
    let text = |read: fn(&CsvSettings) -> &str| {
        move |cx: &App| SharedString::from(read(&AppSettings::global(cx).settings.csv).to_string())
    };

    SettingGroup::new()
        .title(t!("settings.csv.copy_as_csv"))
        .items(vec![
            SettingItem::new(
                t!("settings.csv.delimiter"),
                SettingField::dropdown(
                    CsvDelimiter::ALL
                        .into_iter()
                        .map(|delimiter| (delimiter.as_str().into(), delimiter.label()))
                        .collect(),
                    text(|csv| csv.delimiter.as_str()),
                    setter("csv.delimiter", |csv, value| {
                        csv.delimiter = CsvDelimiter::from_stored(value)
                    }),
                )
                .default_value(SharedString::from(defaults.delimiter.as_str())),
            ),
            SettingItem::new(
                t!("settings.csv.custom_delimiter"),
                SettingField::input(
                    text(|csv| &csv.custom_delimiter),
                    setter("csv.custom_delimiter", |csv, value| {
                        csv.custom_delimiter = value.to_string()
                    }),
                )
                .default_value(SharedString::from(defaults.custom_delimiter.clone())),
            )
            .description(t!("settings.csv.custom_delimiter_description").into_owned()),
            SettingItem::new(
                t!("settings.csv.header_row"),
                SettingField::switch(
                    |cx: &App| AppSettings::global(cx).settings.csv.header,
                    switch_setter("csv.header", |csv, value| csv.header = value),
                )
                .default_value(defaults.header),
            )
            .description(t!("settings.csv.header_row_description").into_owned()),
            SettingItem::new(
                t!("settings.csv.null_text"),
                SettingField::input(
                    text(|csv| &csv.null_text),
                    setter("csv.null_text", |csv, value| {
                        csv.null_text = value.to_string()
                    }),
                )
                .default_value(SharedString::from(defaults.null_text.clone())),
            )
            .description(t!("settings.csv.null_text_description").into_owned()),
            SettingItem::new(
                t!("settings.csv.quote_character"),
                SettingField::input(
                    text(|csv| &csv.quote),
                    setter("csv.quote", |csv, value| csv.quote = value.to_string()),
                )
                .default_value(SharedString::from(defaults.quote.clone())),
            ),
            SettingItem::new(
                t!("settings.csv.quote_values"),
                SettingField::dropdown(
                    CsvQuoting::ALL
                        .into_iter()
                        .map(|quoting| (quoting.as_str().into(), quoting_label(quoting)))
                        .collect(),
                    text(|csv| csv.quoting.as_str()),
                    setter("csv.quoting", |csv, value| {
                        csv.quoting = CsvQuoting::from_stored(value)
                    }),
                )
                .default_value(SharedString::from(defaults.quoting.as_str())),
            )
            .description(t!("settings.csv.quote_values_description").into_owned()),
            SettingItem::new(
                t!("settings.csv.replace_line_breaks"),
                SettingField::switch(
                    |cx: &App| AppSettings::global(cx).settings.csv.replace_line_breaks,
                    switch_setter("csv.replace_line_breaks", |csv, value| {
                        csv.replace_line_breaks = value
                    }),
                )
                .default_value(defaults.replace_line_breaks),
            )
            .description(t!("settings.csv.replace_line_breaks_description").into_owned()),
            SettingItem::new(
                t!("settings.csv.line_break_replacement"),
                SettingField::input(
                    text(|csv| &csv.line_break_replacement),
                    setter("csv.line_break_replacement", |csv, value| {
                        csv.line_break_replacement = value.to_string()
                    }),
                )
                .default_value(SharedString::from(defaults.line_break_replacement.clone())),
            ),
        ])
}

fn quoting_label(quoting: CsvQuoting) -> SharedString {
    match quoting {
        CsvQuoting::Always => t!("settings.csv.quoting_always"),
        CsvQuoting::WhenNeeded => t!("settings.csv.quoting_when_needed"),
        CsvQuoting::Never => t!("settings.csv.quoting_never"),
    }
    .into()
}
