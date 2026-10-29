//! The dialog that sets when a panel's numbers turn yellow and red.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::field,
    input::{Input, InputState},
    notification::NotificationType,
    select::{Select, SelectState},
    v_flex,
};

use super::DashboardTab;
use super::panel::{PanelKind, Thresholds};
use crate::result_ext::ResultExt as _;

pub(super) struct ThresholdForm {
    /// The column a table colors, `None` for a stat.
    column: Option<Entity<SelectState<Vec<String>>>>,
    warning: Entity<InputState>,
    critical: Entity<InputState>,
}

impl ThresholdForm {
    fn new(
        columns: Option<Vec<String>>,
        current: Option<&Thresholds>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let column = columns.map(|columns| {
            let selected = current
                .and_then(|current| current.column.as_ref())
                .and_then(|name| columns.iter().position(|column| column == name))
                .or((!columns.is_empty()).then_some(0));
            cx.new(|cx| SelectState::new(columns, selected.map(IndexPath::new), window, cx))
        });
        let number = |value: Option<f64>,
                      placeholder: &'static str,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                if let Some(value) = value {
                    state.set_value(value.to_string(), window, cx);
                }
                state
            })
        };
        let warning = number(current.map(|current| current.warning), "80", window, cx);
        let critical = number(current.map(|current| current.critical), "95", window, cx);
        Self {
            column,
            warning,
            critical,
        }
    }

    /// The thresholds the fields give, `None` when both are left empty, or
    /// what is wrong with them.
    fn thresholds(&self, cx: &gpui::App) -> Result<Option<Thresholds>, String> {
        let read = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let (warning, critical) = (read(&self.warning), read(&self.critical));
        if warning.is_empty() && critical.is_empty() {
            return Ok(None);
        }
        let parse = |text: &str, name: &str| {
            text.parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("The {name} value is no number."))
        };
        let column = match &self.column {
            Some(select) => Some(
                select
                    .read(cx)
                    .selected_value()
                    .cloned()
                    .ok_or_else(|| "Pick the column to color.".to_string())?,
            ),
            None => None,
        };
        Ok(Some(Thresholds {
            column,
            warning: parse(&warning, "warning")?,
            critical: parse(&critical, "critical")?,
        }))
    }
}

impl Render for ThresholdForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .children(
                self.column
                    .as_ref()
                    .map(|column| field().label("Column").child(Select::new(column).small())),
            )
            .child(
                field()
                    .label("Warning at")
                    .child(Input::new(&self.warning).w_full()),
            )
            .child(
                field()
                    .label("Critical at")
                    .child(Input::new(&self.critical).w_full()),
            )
            .child(div().text_xs().text_color(muted).child(
                "Numbers turn yellow at the warning value and red at the critical one, \
                 green below both. A critical value under the warning one means lower is \
                 worse. Leave both empty to color nothing.",
            ))
    }
}

impl DashboardTab {
    pub(super) fn open_threshold_dialog(
        &mut self,
        panel_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        let panel = self.panels[ix].read(cx);
        let columns = match panel.view.kind() {
            PanelKind::Table => Some(panel.column_names(cx)),
            PanelKind::Stat => None,
            PanelKind::Chart(_) => return,
        };
        let current = panel.view.thresholds().cloned();
        let form = cx.new(|cx| ThresholdForm::new(columns, current.as_ref(), window, cx));
        let tab = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let tab = tab.clone();
            dialog
                .title("Thresholds")
                .w(px(380.))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Save")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let thresholds = match form.read(cx).thresholds(cx) {
                        Ok(thresholds) => thresholds,
                        Err(problem) => {
                            window.push_notification((NotificationType::Warning, problem), cx);
                            return false;
                        }
                    };
                    tab.update(cx, |tab, cx| {
                        tab.set_panel_thresholds(panel_id, thresholds, window, cx)
                    })
                    .log_err();
                    true
                })
        });
    }

    fn set_panel_thresholds(
        &mut self,
        panel_id: i64,
        thresholds: Option<Thresholds>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        self.panels[ix].update(cx, |panel, cx| {
            let view = panel.view.with_thresholds(thresholds);
            panel.set_view(view, window, cx);
        });
        self.save_panel(panel_id, cx);
        cx.notify();
    }
}
