use blanco_ui::IconName;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable, IntoElement, ParentElement, Render, Styled,
    WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Sizable, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, Input, InputState, TabSize},
    notification::NotificationType,
    v_flex,
};
use std::rc::Rc;

use crate::script_completion::ScriptCompletionProvider;

use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::snippets_panel::RefreshSnippets;
use app_database::{AppDatabase, EditorKind, SnippetData};

pub struct SnippetEditor {
    pub snippet_id: Option<i64>,
    pub name: String,
    pub kind: EditorKind,
    /// The word that offers the snippet in completion, optional.
    pub trigger_input: Entity<InputState>,
    pub editor: Entity<EditorState>,
}

impl SnippetEditor {
    pub fn new(kind: EditorKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let trigger_input = cx.new(|cx| InputState::new(window, cx).placeholder("Trigger word"));
        let editor = Self::build_editor(kind, window, cx);

        Self {
            snippet_id: None,
            name: String::new(),
            kind,
            trigger_input,
            editor,
        }
    }

    /// The highlighter language is fixed when an `EditorState` is built, so
    /// switching a snippet between SQL and JavaScript means building a new one.
    fn build_editor(
        kind: EditorKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorState> {
        cx.new(|cx| {
            let editor_settings = &AppSettings::global(cx).settings.editor;
            let folding = editor_settings.folding;
            let tab_size = editor_settings.tab_size;
            let hard_tabs = editor_settings.hard_tabs;
            let mut editor = EditorState::new(window, cx)
                .language(kind.standalone_language().to_string())
                .line_number(true)
                .folding(folding)
                .tab_size(TabSize {
                    tab_size: tab_size as usize,
                    hard_tabs,
                })
                .soft_wrap(true);
            editor.lsp_mut().completion_menu.accept_keys =
                crate::settings::completion_accept_keys(cx);

            if kind == EditorKind::Script {
                // A snippet has no connection behind it, so the completions
                // describe the `db` API in its general form.
                let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                    Rc::new(ScriptCompletionProvider::for_snippet());
                editor.lsp_mut().completion_provider = Some(completion_provider);
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

        self.trigger_input.update(cx, |input, cx| {
            input.replace_all(snippet.trigger.as_deref().unwrap_or_default(), window, cx);
        });

        // Update editor content
        self.editor.update(cx, |editor, cx| {
            editor.replace(&snippet.content, window, cx);
        });

        cx.notify();
    }

    /// Take the snippet's new name after it was renamed in the sidebar.
    pub fn set_name(&mut self, name: &str, cx: &mut Context<Self>) {
        self.name = name.to_string();
        cx.notify();
    }

    /// Save the snippet's text, kind and trigger. Its name and its place in
    /// the sidebar belong to the sidebar, which renames and moves it.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.editor.read(cx).text().to_string();
        let trigger = self.trigger_input.read(cx).value().trim().to_string();
        if trigger.chars().any(char::is_whitespace) {
            window.push_notification(
                (
                    NotificationType::Error,
                    "A trigger is a single word, without spaces.",
                ),
                cx,
            );
            return;
        }
        let trigger = (!trigger.is_empty()).then_some(trigger);
        let kind = self.kind;
        let snippet_id = self.snippet_id;
        let name = match self.name.trim() {
            "" => self.get_title(),
            name => name.to_string(),
        };

        let app_database = AppDatabase::global(cx).clone();
        let save = gpui_tokio::Tokio::spawn_result(cx, async move {
            match snippet_id {
                Some(id) => {
                    app_database
                        .update_snippet_body(id, &content, kind, trigger.as_deref())
                        .await?;
                    Ok(id)
                }
                // A snippet that was never stored goes to the top level.
                None => Ok(app_database
                    .save_snippet(&SnippetData {
                        id: None,
                        name,
                        content,
                        kind,
                        parent_id: None,
                        is_group: false,
                        position: 0,
                        trigger,
                    })
                    .await?),
            }
        });
        cx.spawn(async move |weak_handle: WeakEntity<SnippetEditor>, cx| {
            let Some(id) = save.await.log_err() else {
                return;
            };
            weak_handle
                .update(cx, |editor, _cx| editor.snippet_id = Some(id))
                .log_err();
            cx.update(|cx| cx.dispatch_action(&RefreshSnippets));
        })
        .detach();
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
                Editor::new(&self.editor)
                    .bordered(false)
                    .rounded_none()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(AppSettings::global(cx).settings.editor.font_size))
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
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(160.))
                            .child(Input::new(&self.trigger_input).small()),
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
