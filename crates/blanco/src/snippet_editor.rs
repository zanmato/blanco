use blanco_ui::IconName;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, WeakEntity, Window, actions, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState, TabSize},
    v_flex,
};

use crate::app_database::{AppDatabase, SnippetData};
use crate::snippets_panel::RefreshSnippets;

pub struct SnippetEditor {
    pub id: usize,
    pub snippet_id: Option<i64>,
    pub name: String,
    pub name_input: Entity<InputState>,
    pub editor: Entity<InputState>,
}

impl SnippetEditor {
    pub fn new(id: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Snippet name..."));

        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .soft_wrap(true)
        });

        Self {
            id,
            snippet_id: None,
            name: String::new(),
            name_input,
            editor,
        }
    }

    pub fn load_snippet(
        &mut self,
        snippet: SnippetData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snippet_id = snippet.id;
        self.name = snippet.name.clone();

        // Update name input
        self.name_input.update(cx, |input, cx| {
            input.replace(&snippet.name, window, cx);
        });

        // Update editor content
        self.editor.update(cx, |editor, cx| {
            editor.replace(&snippet.content, window, cx);
        });

        cx.notify();
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).text().to_string();
        let content = self.editor.read(cx).text().to_string();

        if name.trim().is_empty() {
            cx.notify();
            return;
        }

        let snippet = SnippetData {
            id: self.snippet_id,
            name: name.clone(),
            content,
            parent_id: None,
            is_group: false,
            position: 0,
            created_at: 0,
            updated_at: 0,
        };

        let snippet_id = self.snippet_id;

        let app_database = AppDatabase::global(cx).clone();
        cx.spawn(async move |weak_handle: WeakEntity<SnippetEditor>, cx| {
            let saved_id = match app_database.save_snippet(&snippet).await {
                Ok(id) => id,
                Err(err) => {
                    tracing::error!("Failed to save snippet: {}", err);
                    return;
                }
            };

            let saved_name = name.clone();
            let id = if let Some(id) = snippet_id {
                id
            } else {
                saved_id
            };

            // Update the entity with the new snippet ID
            let _ = weak_handle.update(cx, |editor, cx| {
                editor.snippet_id = Some(id);
                editor.name = saved_name.clone();
            });
            let _ = cx.update(|cx| {
                cx.dispatch_action(&RefreshSnippets);
            });
        })
        .detach();
        window.dispatch_action(Box::new(RefreshSnippets), cx);
    }

    pub fn get_title(&self) -> String {
        if self.name.is_empty() {
            "New Snippet".to_string()
        } else {
            self.name.clone()
        }
    }
}

impl Focusable for SnippetEditor {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for SnippetEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .size_full()
            .child(
                // Main code editor area
                Input::new(&self.editor)
                    .bordered(false)
                    .rounded_none()
                    .font_family(cx.theme().mono_font_family.clone())
                    .focus_bordered(false)
                    .size_full(),
            )
            .child(
                // Footer with name input and save button
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .bg(cx.theme().secondary)
                    .items_center()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.name_input)
                                .font_family(cx.theme().mono_font_family.clone()),
                        ),
                    )
                    .child(
                        Button::new("save-snippet")
                            .small()
                            .outline()
                            .icon(IconName::Save)
                            .label("Save Snippet")
                            .on_click(cx.listener(move |this, _event, window, cx| {
                                this.save(window, cx);
                            })),
                    ),
            )
    }
}
