//! Settings page for environment tags: the built-in ones, and the tags the
//! user adds with a name, a colour and whether writes are confirmed first.

use app_database::AppDatabase;
use blanco_core::{EnvironmentTag, EnvironmentType, TAG_COLORS};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Colorize as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    color_picker::{ColorPicker, ColorPickerState},
    h_flex,
    input::{Input, InputState},
    notification::NotificationType,
    setting::{
        RenderOptions, SettingField, SettingFieldElement, SettingGroup, SettingItem, SettingPage,
    },
    v_flex,
};

use crate::connections::tag_color;
use crate::result_ext::ResultExt as _;

pub(super) struct EnvironmentTagsEditor {
    name_input: Entity<InputState>,
    color_picker: Entity<ColorPickerState>,
    confirms_writes: bool,
}

/// The theme colors offered as presets, by the name a tag stores for them.
fn preset_colors(cx: &App) -> Vec<(&'static str, gpui::Hsla)> {
    TAG_COLORS
        .iter()
        .map(|&name| (name, gpui::Hsla::from(tag_color(name, cx))))
        .collect()
}

impl EnvironmentTagsEditor {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let default_color = gpui::Hsla::from(tag_color(TAG_COLORS[1], cx));
        Self {
            name_input: cx.new(|cx| InputState::new(window, cx).placeholder("Tag name")),
            color_picker: cx
                .new(|cx| ColorPickerState::new(window, cx).default_value(default_color)),
            confirms_writes: false,
        }
    }

    /// What a tag stores for the picked color: the name of a preset, so the
    /// tag keeps following the theme, or the color itself as hex.
    fn picked_color(&self, cx: &App) -> String {
        let Some(picked) = self.color_picker.read(cx).value() else {
            return TAG_COLORS[1].to_string();
        };
        preset_colors(cx)
            .into_iter()
            .find(|(_, preset)| *preset == picked)
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| picked.to_hex())
    }

    fn add_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_uppercase();
        if name.is_empty() {
            return;
        }
        if blanco_core::environment_tags()
            .iter()
            .any(|tag| tag.name.eq_ignore_ascii_case(&name))
        {
            window.push_notification(
                (
                    NotificationType::Error,
                    SharedString::from(format!("There is already a tag called {name}.")),
                ),
                cx,
            );
            return;
        }
        let tag = EnvironmentTag {
            id: EnvironmentType::DEV,
            name,
            color: self.picked_color(cx),
            confirms_writes: self.confirms_writes,
        };
        self.name_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.confirms_writes = false;
        Self::save(Some(tag), None, cx);
    }

    /// Save or delete a tag, then reload the tags everywhere.
    fn save(save: Option<EnvironmentTag>, delete: Option<EnvironmentType>, cx: &mut App) {
        let app_database = AppDatabase::global(cx).clone();
        let reload = gpui_tokio::Tokio::spawn_result(cx, async move {
            if let Some(tag) = save {
                app_database.save_environment_tag(&tag).await?;
            }
            if let Some(id) = delete {
                app_database.delete_environment_tag(id).await?;
            }
            Ok(app_database.load_environment_tags().await?)
        });
        cx.spawn(async move |cx| {
            if let Some(tags) = reload.await.log_err() {
                blanco_core::set_custom_environment_tags(tags);
                cx.update(|cx| cx.refresh_windows());
            }
        })
        .detach();
    }

    fn render_tag(tag: &EnvironmentTag, cx: &mut Context<Self>) -> AnyElement {
        let custom = !tag.id.is_built_in();
        let id = tag.id;
        h_flex()
            .gap_2()
            .items_center()
            .child(color_dot(&tag.color, cx))
            .child(div().w(px(120.)).text_sm().child(tag.name.clone()))
            .child(
                Checkbox::new(("tag-confirms-writes", id.to_i32() as usize))
                    .label("Confirm writes")
                    .checked(tag.confirms_writes)
                    .disabled(!custom)
                    .on_click({
                        let tag = tag.clone();
                        move |checked, _, cx| {
                            let tag = EnvironmentTag {
                                confirms_writes: *checked,
                                ..tag.clone()
                            };
                            Self::save(Some(tag), None, cx);
                        }
                    }),
            )
            .when(custom, |this| {
                this.child(div().flex_1()).child(
                    Button::new(("tag-delete", id.to_i32() as usize))
                        .label("Delete")
                        .ghost()
                        .xsmall()
                        .on_click(move |_, _, cx| Self::save(None, Some(id), cx)),
                )
            })
            .into_any_element()
    }
}

fn color_dot(color: &str, cx: &App) -> impl IntoElement {
    div()
        .size(px(10.))
        .rounded_full()
        .flex_none()
        .bg(tag_color(color, cx))
}

impl Render for EnvironmentTagsEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tags = blanco_core::environment_tags();
        v_flex()
            .gap_2()
            .children(tags.iter().map(|tag| Self::render_tag(tag, cx)))
            .child(
                h_flex()
                    .pt_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(140.))
                            .child(Input::new(&self.name_input).small()),
                    )
                    .child(
                        ColorPicker::new(&self.color_picker)
                            .small()
                            .featured_colors(
                                preset_colors(cx)
                                    .into_iter()
                                    .map(|(_, color)| color)
                                    .collect(),
                            ),
                    )
                    .child(
                        Checkbox::new("new-tag-confirms-writes")
                            .label("Confirm writes")
                            .checked(self.confirms_writes)
                            .on_click(cx.listener(|editor, checked, _, cx| {
                                editor.confirms_writes = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tag-add").label("Add").small().on_click(
                            cx.listener(|editor, _, window, cx| editor.add_tag(window, cx)),
                        ),
                    ),
            )
    }
}

struct EnvironmentTagsField(Entity<EnvironmentTagsEditor>);

impl SettingFieldElement for EnvironmentTagsField {
    type Element = AnyElement;

    fn render_field(&self, _: &RenderOptions, _: &mut Window, _: &mut App) -> Self::Element {
        self.0.clone().into_any_element()
    }
}

pub(super) fn environments_page(editor: Entity<EnvironmentTagsEditor>) -> SettingPage {
    SettingPage::new("Environments").groups(vec![
        SettingGroup::new().title("Tags").items(vec![
            SettingItem::new("Environment Tags", SettingField::element(EnvironmentTagsField(editor)))
                .description(
                    "Tag connections with an environment. A tag that confirms writes asks before running a statement that modifies data, committing grid edits or running a script, as PROD does.",
                ),
        ]),
    ])
}
