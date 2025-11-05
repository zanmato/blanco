use gpui::{
    div, px, App, AppContext, Context, DismissEvent, Entity, EventEmitter, IntoElement,
    ParentElement, Render, Styled, Window,
};
use gpui_component::{
    form::form_field,
    input::{InputState, TextInput},
    v_flex, StyledExt,
};
use log::error;

use crate::app_events::AppEvent;

pub struct RenameTabForm {
    pub input: Entity<InputState>,
    pub tab_index: usize,
    current_name: String,
}

impl RenameTabForm {
    pub fn new(
        tab_index: usize,
        current_name: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                // Set the initial value to the current name
                state.set_value(current_name.clone(), window, cx);
                state
            });

            Self {
                input,
                tab_index,
                current_name,
            }
        })
    }

    pub fn get_value(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn validate(&self, cx: &App) -> Option<String> {
        let new_name = self.get_value(cx);

        if new_name.trim().is_empty() {
            return Some("Tab name cannot be empty".to_string());
        }

        if new_name == self.current_name {
            return Some("New name is the same as current name".to_string());
        }

        None
    }

    pub fn rename_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        let current_value = self.get_value(cx);
        log::info!(
            "RenameTabForm::rename_tab called - tab_index={}, current_value='{}'",
            self.tab_index,
            current_value
        );

        if let Some(error) = self.validate(cx) {
            error!("Validation failed: {}", error);
            // TODO: Show notification to user - for now just log and return
            return;
        }

        let new_name = self.get_value(cx);
        let tab_index = self.tab_index;

        log::info!(
            "Emitting RenameTabRequested - tab_index={}, new_name='{}'",
            tab_index,
            new_name
        );

        // Emit dismiss event to close popover
        cx.emit(DismissEvent);

        // Emit rename event to handle the tab renaming
        cx.emit(AppEvent::RenameTabRequested {
            tab_index,
            new_name,
        });
    }
}

impl EventEmitter<DismissEvent> for RenameTabForm {}
impl EventEmitter<AppEvent> for RenameTabForm {}

impl Render for RenameTabForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3().min_w(px(200.)).child(
            form_field()
                .label("Name")
                .child(TextInput::new(&self.input).w_full()),
        )
    }
}
