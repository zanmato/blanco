use blanco_ui::IconName;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable, IntoElement, ParentElement, Render, Styled,
    WeakEntity, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, TabSize},
    v_flex,
};
use std::rc::Rc;

use crate::script_completion::ScriptCompletionProvider;

use crate::app_database::{AppDatabase, EditorKind, SnippetData};
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::snippets_panel::RefreshSnippets;

pub struct SnippetEditor {
    pub snippet_id: Option<i64>,
    pub name: String,
    pub kind: EditorKind,
    pub name_input: Entity<InputState>,
    pub editor: Entity<InputState>,
}

impl SnippetEditor {
    pub fn new(kind: EditorKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Snippet name..."));
        let editor = Self::build_editor(kind, window, cx);

        Self {
            snippet_id: None,
            name: String::new(),
            kind,
            name_input,
            editor,
        }
    }

    /// The highlighter language is fixed when an `InputState` is built, so
    /// switching a snippet between SQL and JavaScript means building a new one.
    fn build_editor(
        kind: EditorKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        cx.new(|cx| {
            let editor_settings = &AppSettings::global(cx).settings.editor;
            let folding = editor_settings.folding;
            let tab_size = editor_settings.tab_size;
            let hard_tabs = editor_settings.hard_tabs;
            let mut editor = InputState::new(window, cx)
                .code_editor(kind.standalone_language().to_string())
                .line_number(true)
                .folding(folding)
                .tab_size(TabSize {
                    tab_size: tab_size as usize,
                    hard_tabs,
                })
                .soft_wrap(true);

            if kind == EditorKind::Script {
                // A snippet has no connection behind it, so the completions
                // describe the `db` API in its general form.
                let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                    Rc::new(ScriptCompletionProvider::for_snippet());
                editor.lsp.completion_provider = Some(completion_provider);
            }

            editor
        })
    }

    /// Switch the snippet between SQL and JavaScript, carrying its text over.
    pub fn set_kind(&mut self, kind: EditorKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.kind == kind {
            return;
        }
        let content = self.editor.read(cx).text().to_string();
        self.kind = kind;
        self.editor = Self::build_editor(kind, window, cx);
        self.editor.update(cx, |editor, cx| {
            editor.replace(&content, window, cx);
        });
        cx.notify();
    }

    pub fn load_snippet(
        &mut self,
        snippet: SnippetData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snippet_id = snippet.id;
        self.name = snippet.name.clone();
        self.set_kind(snippet.kind, window, cx);

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
            kind: self.kind,
            parent_id: None,
            is_group: false,
            position: 0,
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
            weak_handle
                .update(cx, |editor, _cx| {
                    editor.snippet_id = Some(id);
                    editor.name = saved_name.clone();
                })
                .log_err();
            cx.update(|cx| {
                cx.dispatch_action(&RefreshSnippets);
            });
        })
        .detach();
        window.dispatch_action(Box::new(RefreshSnippets), cx);
    }

    /// SQL/Script switch in the footer. Which one a snippet is decides its
    /// highlighting and completions, and where it makes sense to insert it.
    fn render_kind_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_1().children(
            [EditorKind::Query, EditorKind::Script]
                .into_iter()
                .map(|kind| {
                    Button::new(("snippet-kind", kind as usize))
                        .small()
                        .outline()
                        .label(kind.label())
                        .when(self.kind == kind, |button| button.primary())
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            this.set_kind(kind, window, cx);
                        }))
                }),
        )
    }

    pub fn get_title(&self) -> String {
        if self.name.is_empty() {
            match self.kind {
                EditorKind::Query => "New Snippet".to_string(),
                EditorKind::Script => "New Script Snippet".to_string(),
            }
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
                    .p_2()
                    .border_t_1()
                    .bg(cx.theme().title_bar)
                    .border_color(cx.theme().border)
                    // Flush against the bottom of the editor card; the content
                    // mask is rectangular, so round the corners here too.
                    .rounded_b(crate::app::PANEL_RADIUS)
                    .items_center()
                    .child(self.render_kind_toggle(cx))
                    .child(div().flex_1().child(Input::new(&self.name_input).small()))
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
