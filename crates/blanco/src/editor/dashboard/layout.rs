//! Moving panels by dragging their header and sizing them by dragging their
//! corner, both snapped to the dashboard's grid.

use app_database::{AppDatabase, DASHBOARD_COLUMNS};
use gpui::{
    App, AppContext as _, Context, DragMoveEvent, IntoElement, ParentElement, Pixels, Point,
    Render, SharedString, Styled, Window, div, px,
};
use gpui_component::ActiveTheme as _;

use super::{DashboardTab, ROW_HEIGHT};

/// The tallest a panel gets, in grid rows.
pub(super) const MAX_PANEL_HEIGHT: u8 = 12;

/// A panel being dragged by its header to another place.
#[derive(Clone)]
pub(super) struct DraggedPanel {
    pub panel_id: i64,
    pub title: SharedString,
}

impl Render for DraggedPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .text_sm()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.title.clone())
    }
}

/// A panel being sized by its corner. It draws nothing while dragged: the
/// panel itself grows and shrinks.
#[derive(Clone)]
pub(super) struct ResizingPanel {
    pub panel_id: i64,
}

impl Render for ResizingPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Where a resize started: the pointer and the panel's size then.
#[derive(Clone, Copy)]
pub(super) struct ResizeStart {
    pub panel_id: i64,
    pub position: Point<Pixels>,
    pub width: u8,
    pub height: u8,
}

/// The size a resize gives a panel that was `start` sized when the pointer
/// moved by `delta`, on a grid whose columns are `column_width` wide.
pub(super) fn resized(start: ResizeStart, delta: Point<Pixels>, column_width: Pixels) -> (u8, u8) {
    let steps = |delta: Pixels, step: Pixels| -> i32 {
        if step <= px(0.) {
            0
        } else {
            (delta / step).round() as i32
        }
    };
    let width = (i32::from(start.width) + steps(delta.x, column_width))
        .clamp(1, i32::from(DASHBOARD_COLUMNS));
    let height = (i32::from(start.height) + steps(delta.y, px(ROW_HEIGHT)))
        .clamp(1, i32::from(MAX_PANEL_HEIGHT));
    (width as u8, height as u8)
}

impl DashboardTab {
    /// Put the panel at `target` in the layout order.
    pub(super) fn move_panel_to(&mut self, panel_id: i64, target: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        let target = target.min(self.panels.len().saturating_sub(1));
        if ix == target {
            return;
        }
        let panel = self.panels.remove(ix);
        self.panels.insert(target, panel);
        self.save_order(cx);
        cx.notify();
    }

    pub(super) fn save_order(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .panels
            .iter()
            .map(|panel| panel.read(cx).data.id)
            .collect();
        let database = AppDatabase::global(cx).clone();
        let dashboard_id = self.dashboard_id;
        cx.background_spawn(async move {
            if let Err(error) = database.reorder_dashboard_panels(dashboard_id, &ids).await {
                tracing::error!("Failed to reorder the dashboard panels: {error}");
            }
        })
        .detach();
    }

    pub(super) fn start_resize(&mut self, panel_id: i64, position: Point<Pixels>, cx: &App) {
        let Some(ix) = self.panel_index(panel_id, cx) else {
            return;
        };
        let data = &self.panels[ix].read(cx).data;
        self.resize_start = Some(ResizeStart {
            panel_id,
            position,
            width: data.width,
            height: data.height,
        });
    }

    /// Size the panel being resized to where the pointer is, snapped to the
    /// grid whose bounds the event carries.
    pub(super) fn on_resize_move(
        &mut self,
        event: &DragMoveEvent<ResizingPanel>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(start) = self
            .resize_start
            .filter(|start| start.panel_id == event.drag(cx).panel_id)
        else {
            return;
        };
        let column_width = event.bounds.size.width / f32::from(DASHBOARD_COLUMNS);
        let delta = event.event.position - start.position;
        let (width, height) = resized(start, delta, column_width);
        let Some(ix) = self.panel_index(start.panel_id, cx) else {
            return;
        };
        self.panels[ix].update(cx, |panel, cx| {
            if panel.data.width != width || panel.data.height != height {
                panel.data.width = width;
                panel.data.height = height;
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Save the size a resize ended on, once the drag is over.
    pub(super) fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if cx.has_active_drag() {
            return;
        }
        let Some(start) = self.resize_start.take() else {
            return;
        };
        self.save_panel(start.panel_id, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    #[test]
    fn a_resize_snaps_to_the_grid_and_stays_on_it() {
        let start = ResizeStart {
            panel_id: 1,
            position: point(px(0.), px(0.)),
            width: 6,
            height: 4,
        };
        let column = px(100.);
        assert_eq!(resized(start, point(px(140.), px(80.)), column), (7, 5));
        assert_eq!(resized(start, point(px(-40.), px(-20.)), column), (6, 4));
        assert_eq!(
            resized(start, point(px(-5000.), px(-5000.)), column),
            (1, 1)
        );
        assert_eq!(
            resized(start, point(px(5000.), px(5000.)), column),
            (DASHBOARD_COLUMNS, MAX_PANEL_HEIGHT)
        );
    }
}
