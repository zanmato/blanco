use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, px,
};
use gpui_component::{
    form::field,
    input::{Input, InputState},
    v_flex,
};

pub struct RenameTabForm {
    pub input: Entity<InputState>,
}

impl RenameTabForm {
    pub fn new(current_name: String, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                // Set the initial value to the current name
                state.set_value(current_name.clone(), window, cx);
                state
            });

            Self { input }
        })
    }

    pub fn get_value(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }
}

impl Render for RenameTabForm {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3().min_w(px(200.)).child(
            field()
                .label("Name")
                .child(Input::new(&self.input).w_full()),
        )
    }
}
