//! SQL Completion Popup UI
//!
//! This module provides the GPUI-based completion popup that shows SQL suggestions
//! to the user as they type.

use gpui::{
    div, prelude::FluentBuilder, px, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Keystroke, ParentElement, Pixels, Point, Render, Styled, Window,
    InteractiveElement, StatefulInteractiveElement,
};
use gpui_component::{
    v_flex, h_flex, ActiveTheme,
};
use crate::sql_completion::{CompletionItem, CompletionResult};

/// Events emitted by the completion popup
#[derive(Debug, Clone)]
pub enum CompletionPopupEvent {
    /// User selected a completion item
    ItemSelected(CompletionItem),
    /// User cancelled completion (ESC key, clicked outside, etc.)
    CompletionCancelled,
    /// User wants to trigger completion manually (Ctrl+Space)
    TriggerCompletion,
}

/// State for the completion popup
pub struct CompletionPopup {
    /// Handle to the window for focus management
    focus_handle: FocusHandle,
    /// Current completion results
    completion_result: Option<CompletionResult>,
    /// Currently selected item index
    selected_index: usize,
    /// Position where the popup should appear
    position: Point<Pixels>,
    /// Whether the popup is visible
    visible: bool,
    /// Trigger character that opened this popup
    trigger_character: Option<char>,
    /// Current text before cursor (for filtering)
    current_word: Option<String>,
}

impl CompletionPopup {
    /// Create a new completion popup
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            completion_result: None,
            selected_index: 0,
            position: Point::new(px(0.0), px(0.0)),
            visible: false,
            trigger_character: None,
            current_word: None,
        }
    }

    /// Show the completion popup with new results
    pub fn show(&mut self, result: CompletionResult, position: Point<Pixels>, trigger_character: Option<char>, current_word: Option<String>) {
        self.completion_result = Some(result);
        self.position = position;
        self.trigger_character = trigger_character;
        self.current_word = current_word;
        self.selected_index = 0;
        self.visible = true;

        // Clamp selected index to valid range
        if let Some(ref result) = self.completion_result {
            if self.selected_index >= result.items.len() && !result.items.is_empty() {
                self.selected_index = result.items.len() - 1;
            }
        }
    }

    /// Hide the completion popup
    pub fn hide(&mut self) {
        self.visible = false;
        self.completion_result = None;
        self.selected_index = 0;
        self.trigger_character = None;
        self.current_word = None;
    }

    /// Check if the popup is currently visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the currently selected completion item
    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.completion_result
            .as_ref()
            .and_then(|result| result.items.get(self.selected_index))
    }

    /// Move selection up
    pub fn move_selection_up(&mut self) {
        if let Some(ref result) = self.completion_result {
            if !result.items.is_empty() {
                if self.selected_index == 0 {
                    self.selected_index = result.items.len() - 1;
                } else {
                    self.selected_index -= 1;
                }
            }
        }
    }

    /// Move selection down
    pub fn move_selection_down(&mut self) {
        if let Some(ref result) = self.completion_result {
            if !result.items.is_empty() {
                self.selected_index = (self.selected_index + 1) % result.items.len();
            }
        }
    }

    /// Select the current item and emit event
    pub fn select_current(&self, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_item() {
            cx.emit(CompletionPopupEvent::ItemSelected(item.clone()));
        }
    }

    /// Update completion results and filter by current word
    pub fn update_completions(&mut self, result: CompletionResult, current_word: Option<String>) {
        self.completion_result = Some(result);
        self.current_word = current_word.clone();
        self.selected_index = 0;

        // Filter items by current word if provided
        if let Some(ref word) = current_word {
            if let Some(ref mut result) = self.completion_result {
                result.items.retain(|item| {
                    item.label.to_lowercase().starts_with(&word.to_lowercase())
                });
            }
        }

        // Reset selection if needed
        if let Some(ref result) = self.completion_result {
            if self.selected_index >= result.items.len() && !result.items.is_empty() {
                self.selected_index = 0;
            }
        }

        // Hide if no items
        if let Some(ref result) = self.completion_result {
            if result.items.is_empty() {
                self.hide();
            }
        }
    }

    /// Calculate the optimal size and position for the popup
    fn calculate_popup_bounds(&self, window: &mut Window, cx: &mut Context<Self>) -> (Point<Pixels>, Pixels, Pixels) {
        let window_bounds = window.bounds();
        let max_width = px(400.0);
        let max_height = px(300.0);

        // Position popup below the cursor
        let mut position = self.position;
        let x = position.x;
        let y = position.y + px(20.0); // Offset below cursor line

        // Adjust if popup would go outside window bounds
        if x + max_width > window_bounds.size.width {
            position.x = (window_bounds.size.width - max_width).max(px(0.0));
        }

        if y + max_height > window_bounds.size.height {
            // Show above cursor instead
            position.y = (self.position.y - max_height - px(20.0)).max(px(0.0));
        } else {
            position.y = y;
        }

        (position, max_width, max_height)
    }

    /// Handle keyboard input
    pub fn handle_keydown(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        if !self.visible {
            return false;
        }

        match keystroke.key.as_str() {
            "escape" => {
                self.hide();
                cx.emit(CompletionPopupEvent::CompletionCancelled);
                true
            }
            "up" | "ctrl-p" => {
                self.move_selection_up();
                true
            }
            "down" | "ctrl-n" => {
                self.move_selection_down();
                true
            }
            "enter" | "tab" => {
                self.select_current(cx);
                true
            }
            "ctrl-space" => {
                cx.emit(CompletionPopupEvent::TriggerCompletion);
                true
            }
            _ => false,
        }
    }
}

impl EventEmitter<CompletionPopupEvent> for CompletionPopup {}
impl Focusable for CompletionPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CompletionPopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible || self.completion_result.is_none() {
            return div().invisible();
        }

        let result = self.completion_result.as_ref().unwrap();
        if result.items.is_empty() {
            return div().invisible();
        }

        let (position, width, height) = self.calculate_popup_bounds(window, cx);

        div()
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(width)
            .max_h(height)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .child(
                v_flex()
                    .w_full()
                    .children(
                        result.items.iter().enumerate().map(|(index, item)| {
                            let is_selected = index == self.selected_index;

                            // Render completion item
                            div()
                                .id(("completion-item", index))
                                .w_full()
                                .px_3()
                                .py_2()
                                .when(is_selected, |div| {
                                    div.bg(cx.theme().primary.opacity(0.1))
                                        .border_l_2()
                                        .border_color(cx.theme().primary)
                                })
                                .on_click(cx.listener(move |this, _event, window, cx| {
                                    // Select this item
                                    this.selected_index = index;
                                    this.select_current(cx);
                                }))
                                .child(
                                    h_flex()
                                        .w_full()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    // Icon based on completion kind
                                                    div()
                                                        .w_4()
                                                        .h_4()
                                                        .rounded(px(4.0))
                                                        .bg(match item.kind {
                                                            crate::sql_completion::CompletionItemKind::Table => cx.theme().blue,
                                                            crate::sql_completion::CompletionItemKind::Column => cx.theme().green,
                                                            crate::sql_completion::CompletionItemKind::Schema => cx.theme().blue,
                                                            crate::sql_completion::CompletionItemKind::Keyword => cx.theme().primary,
                                                            crate::sql_completion::CompletionItemKind::Function => cx.theme().blue,
                                                            crate::sql_completion::CompletionItemKind::Alias => cx.theme().muted,
                                                        })
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(cx.theme().background)
                                                                .font_family("Fira Code")
                                                                .justify_center()
                                                                .child(match item.kind {
                                                                    crate::sql_completion::CompletionItemKind::Table => "T",
                                                                    crate::sql_completion::CompletionItemKind::Column => "C",
                                                                    crate::sql_completion::CompletionItemKind::Schema => "S",
                                                                    crate::sql_completion::CompletionItemKind::Keyword => "K",
                                                                    crate::sql_completion::CompletionItemKind::Function => "F",
                                                                    crate::sql_completion::CompletionItemKind::Alias => "A",
                                                                })
                                                        )
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_family("Fira Code")
                                                        .text_color(cx.theme().foreground)
                                                        .child(item.label.clone())
                                                )
                                        )
                                        .when_some(item.detail.as_ref(), |parent_div, detail| {
                                            parent_div.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .font_family("Fira Code")
                                                    .child(detail.clone())
                                            )
                                        })
                                )
                        })
                    )
            )
    }
}

/// Manager for handling completion popup lifecycle
pub struct CompletionPopupManager {
    popup: Option<Entity<CompletionPopup>>,
}

impl CompletionPopupManager {
    pub fn new() -> Self {
        Self { popup: None }
    }

    /// Create or get the completion popup
    pub fn get_or_create_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<CompletionPopup> {
        if self.popup.is_none() {
            self.popup = Some(cx.new(|cx| CompletionPopup::new(cx)));
        }
        self.popup.as_ref().unwrap().clone()
    }

    /// Show completion popup with results
    pub fn show_completions(&mut self, result: CompletionResult, position: Point<Pixels>, trigger_character: Option<char>, current_word: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let popup = self.get_or_create_popup(window, cx);
        popup.update(cx, |popup, cx| {
            popup.show(result, position, trigger_character, current_word);
        });
    }

    /// Hide the completion popup
    pub fn hide_popup(&mut self, cx: &mut Context<Self>) {
        if let Some(ref popup) = self.popup {
            popup.update(cx, |popup, cx| {
                popup.hide();
            });
        }
    }

    /// Update completion results
    pub fn update_completions(&mut self, result: CompletionResult, current_word: Option<String>, cx: &mut Context<Self>) {
        if let Some(ref popup) = self.popup {
            popup.update(cx, |popup, cx| {
                popup.update_completions(result, current_word);
            });
        }
    }

    /// Check if popup is visible
    pub fn is_popup_visible(&self, cx: &App) -> bool {
        if let Some(ref popup) = self.popup {
            popup.read(cx).is_visible()
        } else {
            false
        }
    }

    /// Handle keyboard input for the popup
    pub fn handle_keydown(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        if let Some(ref popup) = self.popup {
            popup.update(cx, |popup, cx| {
                popup.handle_keydown(keystroke, cx)
            })
        } else {
            false
        }
    }

    /// Get selected item
    pub fn get_selected_item(&self, cx: &App) -> Option<CompletionItem> {
        if let Some(ref popup) = self.popup {
            popup.read(cx).selected_item().cloned()
        } else {
            None
        }
    }
}

impl Default for CompletionPopupManager {
    fn default() -> Self {
        Self::new()
    }
}