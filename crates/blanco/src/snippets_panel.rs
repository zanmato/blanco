use crate::app::{NewSnippet, OpenSnippetEditor};
use crate::app_database::{AppDatabase, SnippetData};
use crate::app_events::AppEvent;
use crate::snippets_panel_delegate::{SnippetItemMetadata, SnippetsTreeDelegate};
use blanco_ui::draggable_tree::{DraggableTreeState, TreeItem};
use gpui::{
    AppContext, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Styled, Window, actions,
    prelude::FluentBuilder, px,
};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    ActiveTheme as _, StyledExt, h_flex, input::InputState, label::Label, v_flex,
};

actions!(snippets, [CreateGroup, RefreshSnippets]);

impl EventEmitter<AppEvent> for SnippetsPanel {}

pub struct SnippetsPanel {
    pub snippets: Vec<SnippetData>,
    tree_state: Entity<DraggableTreeState<SnippetsTreeDelegate>>,
    creating_group: bool,
    group_name_input: Option<Entity<InputState>>,
}

impl SnippetsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app_database = AppDatabase::global(cx);
        let snippets = smol::block_on(async {
            match app_database.load_snippets().await {
                Ok(snippets) => snippets,
                Err(e) => {
                    tracing::error!("Failed to load snippets: {}", e);
                    vec![]
                }
            }
        });

        let panel_entity = cx.entity();
        let delegate = SnippetsTreeDelegate::new(&panel_entity);
        let tree_state = cx.new(|cx| DraggableTreeState::new(delegate, cx));

        let mut panel = Self {
            snippets,
            tree_state,
            creating_group: false,
            group_name_input: None,
        };

        panel.refresh_tree(cx);
        panel
    }

    pub fn refresh_snippets(&mut self, cx: &mut Context<Self>) {
        tracing::info!(
            "Refreshing snippets panel, currently have {} snippets",
            self.snippets.len()
        );
        let app_database = AppDatabase::global(cx);
        self.snippets = smol::block_on(async {
            match app_database.load_snippets().await {
                Ok(snippets) => {
                    tracing::info!("Loaded {} snippets from database", snippets.len());
                    snippets
                }
                Err(e) => {
                    tracing::error!("Failed to load snippets: {}", e);
                    vec![]
                }
            }
        });

        self.refresh_tree(cx);
        cx.notify();
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let tree_items = self.build_tree_items();
        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
        });
    }

    fn build_tree_items(&self) -> Vec<TreeItem> {
        let root_items: Vec<&SnippetData> = self
            .snippets
            .iter()
            .filter(|s| s.parent_id.is_none())
            .collect();

        root_items
            .into_iter()
            .map(|snippet| self.build_tree_item(snippet, &self.snippets))
            .collect()
    }

    fn build_tree_item(&self, snippet: &SnippetData, all_snippets: &[SnippetData]) -> TreeItem {
        let id = SharedString::from(format!("snippet:{}", snippet.id.unwrap_or(0)));
        let label = SharedString::from(snippet.name.clone());
        let _metadata = SnippetItemMetadata {
            id: snippet.id.unwrap_or(0),
            name: snippet.name.clone(),
            is_group: snippet.is_group,
            parent_id: snippet.parent_id,
        };

        let mut item = TreeItem::new(id.clone(), label.clone()).expanded(true);

        if snippet.is_group {
            let children: Vec<TreeItem> = all_snippets
                .iter()
                .filter(|s| s.parent_id == snippet.id)
                .map(|child| self.build_tree_item(child, all_snippets))
                .collect();

            for child in children {
                item = item.child(child);
            }
        }

        item
    }

    pub fn handle_snippet_double_click(
        &mut self,
        item_id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id_str) = item_id.strip_prefix("snippet:")
            && let Ok(id) = id_str.parse::<i64>()
            && let Some(snippet) = self.snippets.iter().find(|s| s.id == Some(id))
        {
            if snippet.is_group {
                // The draggable tree handles this automatically
            } else {
                cx.emit(AppEvent::OpenSnippetEditor {
                    snippet_id: Some(id),
                });
            }
        }
    }

    pub fn copy_snippet_to_clipboard(&mut self, snippet_id: i64, cx: &mut Context<Self>) {
        if let Some(snippet) = self.snippets.iter().find(|s| s.id == Some(snippet_id)) {
            cx.write_to_clipboard(ClipboardItem::new_string(snippet.content.clone()));
        }
    }

    pub fn delete_snippet(
        &mut self,
        snippet_id: i64,
        _snippet_name: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_database = AppDatabase::global(cx);

        smol::block_on(async {
            match app_database.delete_snippet(snippet_id).await {
                Ok(_) => {
                    tracing::info!("Deleted snippet: {}", snippet_id);
                }
                Err(e) => {
                    tracing::error!("Failed to delete snippet: {}", e);
                }
            }
        });

        self.refresh_snippets(cx);
        cx.emit(AppEvent::SnippetDeleted { id: snippet_id });
    }

    pub fn handle_drop(
        &mut self,
        snippet_id: i64,
        new_parent_id: Option<i64>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_database = AppDatabase::global(cx).clone();

        let position = if let Some(parent_id) = new_parent_id {
            self.snippets
                .iter()
                .filter(|s| s.parent_id == Some(parent_id))
                .map(|s| s.position)
                .max()
                .unwrap_or(0)
                + 1
        } else {
            self.snippets
                .iter()
                .filter(|s| s.parent_id.is_none())
                .map(|s| s.position)
                .max()
                .unwrap_or(0)
                + 1
        };

        cx.spawn(async move |weak_panel, cx: &mut gpui::AsyncApp| {
            match app_database
                .move_snippet(snippet_id, new_parent_id, position)
                .await
            {
                Ok(_) => {
                    tracing::info!("Moved snippet {} to parent {:?}", snippet_id, new_parent_id);
                }
                Err(e) => {
                    tracing::error!("Failed to move snippet: {}", e);
                }
            }

            // Defer the tree refresh to avoid nested update panic
            let _ = weak_panel.update(cx, |this, cx| {
                this.refresh_snippets(cx);
            });
        })
        .detach();
    }

    pub fn create_new_group(&mut self, cx: &mut Context<Self>) {
        let name = if let Some(input) = &self.group_name_input {
            input.read(cx).text().to_string()
        } else {
            String::new()
        };

        if name.trim().is_empty() {
            self.creating_group = false;
            self.group_name_input = None;
            cx.notify();
            return;
        }

        let app_database = AppDatabase::global(cx);
        let new_group = SnippetData {
            id: None,
            name: name.clone(),
            content: String::new(),
            parent_id: None,
            is_group: true,
            position: 0,
            created_at: 0,
            updated_at: 0,
        };

        smol::block_on(async {
            match app_database.save_snippet(&new_group).await {
                Ok(id) => {
                    tracing::info!("Created new group '{}' with id {}", name, id);
                }
                Err(e) => {
                    tracing::error!("Failed to create group: {}", e);
                }
            }
        });

        self.creating_group = false;
        self.group_name_input = None;
        self.refresh_snippets(cx);
    }

    pub fn start_creating_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_group = true;
        self.group_name_input =
            Some(cx.new(|cx| InputState::new(window, cx).placeholder("Group name...")));
        cx.notify();
    }
}

impl Render for SnippetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let group_input = self.group_name_input.clone();
        let creating_group = self.creating_group;

        v_flex()
            .id("snippets-panel")
            .min_h_72()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .on_action(cx.listener(|_this, _: &NewSnippet, _window, cx| {
                cx.emit(AppEvent::OpenSnippetEditor { snippet_id: None });
            }))
            .on_action(
                cx.listener(|_this, action: &OpenSnippetEditor, _window, cx| {
                    cx.emit(AppEvent::OpenSnippetEditor {
                        snippet_id: action.snippet_id,
                    });
                }),
            )
            .on_action(cx.listener(|this, _: &CreateGroup, window, cx| {
                this.start_creating_group(window, cx);
            }))
            .child(
                h_flex()
                    .px_3()
                    .py(px(6.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new("Snippets")
                            .font_bold()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                v_flex()
                    .id("snippets-tree-container")
                    .h_full()
                    .when_some(group_input, |this, input| {
                        this.child(
                            h_flex()
                                .px_3()
                                .py_1()
                                .gap_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().background)
                                .child(Input::new(&input))
                                .when(creating_group, |div| {
                                    div.on_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, _window, cx| {
                                            let key = event.keystroke.key.as_str();
                                            match key {
                                                "enter" => {
                                                    this.create_new_group(cx);
                                                }
                                                "escape" => {
                                                    this.creating_group = false;
                                                    this.group_name_input = None;
                                                    cx.notify();
                                                }
                                                _ => {}
                                            }
                                        },
                                    ))
                                }),
                        )
                    })
                    .child(self.tree_state.clone())
                    .overflow_y_scrollbar(),
            )
    }
}
