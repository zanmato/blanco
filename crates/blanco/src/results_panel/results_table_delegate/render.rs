use std::ops::Range;

use gpui::AppContext as _;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Context, CursorStyle, DragMoveEvent, Entity, FontWeight, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, PathBuilder, Render,
    StatefulInteractiveElement, Styled, StyledText, Window, canvas, div, point, px,
};
use gpui_component::popover::{Popover, PopoverState};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Selectable, Sizable, Size,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    h_flex,
    input::{Editor, EditorState, Input, InputState, MoveLeft, MoveRight},
    menu::{PopupMenu, PopupMenuItem},
    table::{Column, ColumnSort, TableDelegate, TableState},
    tooltip::Tooltip,
    v_flex,
};

use blanco_core::connection_trait::ColumnType;

use super::{ResultsTableDelegate, results_settings};
use crate::app::{
    AddRow, ClearCompareSelection, CompareCellWithSelected, CompareRowWithSelected, CopyAsCSV,
    CopyAsHTML, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, CopyAsTSV, CopyAsVALUES, CopyCellValue,
    CopyColumnValues, DeleteRow, DuplicateRow, ExportAsCSV, ExportAsHTML, ExportAsJSON,
    ExportAsMarkdown, ExportAsSQL, ExportAsTSV, FilterByCellValue, HideResultColumn,
    OpenReferencedRow, PinResultColumn, SelectCellForCompare, SelectRowForCompare, SetCellDefault,
    SetCellNull, ToggleCellExpression,
};
use crate::results_panel::cell_edit_state::{
    CellInput, DisplayText, compare_numeric, display_preview, display_text,
};
use crate::results_panel::compare::CompareKind;
use crate::results_panel::foreign_key_popover::ForeignKeyPopover;
use crate::results_panel::temporal::TemporalShape;
use blanco_ui::IconName;

/// How a cell reads while its value is on the server: the start of the
/// value and its full size.
fn deferred_display_text(
    preview: &str,
    size: u64,
    loading: bool,
    show_invisible: bool,
) -> DisplayText {
    const SHOWN_CHARACTERS: usize = 48;
    let mut shown = display_preview(preview, SHOWN_CHARACTERS, show_invisible);
    let size = blanco_core::binary::format_size(usize::try_from(size).unwrap_or(usize::MAX));
    let state = if loading { ", loading" } else { "" };
    shown.text.push_str(&format!(" ({size}{state})"));
    shown
}

/// A cell's text, with its marks for invisible characters tinted the way
/// VS Code boxes them.
fn render_display_text(
    shown: DisplayText,
    cx: &Context<TableState<ResultsTableDelegate>>,
) -> AnyElement {
    if shown.markers.is_empty() {
        return shown.text.into_any_element();
    }
    let style = HighlightStyle {
        background_color: Some(cx.theme().yellow.opacity(0.35)),
        ..HighlightStyle::default()
    };
    StyledText::new(shown.text)
        .with_highlights(shown.markers.into_iter().map(|range| (range, style)))
        .into_any_element()
}

/// Drag payload for the expanded cell editor's resize grip.
#[derive(Clone)]
struct ResizeExpandedCell;

impl Render for ResizeExpandedCell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Two diagonal strokes hugging the bottom-left corner, mirroring the browser's
/// textarea resize affordance so the drag target is discoverable. The strokes
/// run perpendicular to the drag axis, so they mirror the familiar bottom-right
/// grip rather than repeating it.
fn render_resize_grip(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let inset = px(2.5);
            let extent = bounds.size.width.min(bounds.size.height) - inset * 2.;
            if extent <= px(0.) {
                return;
            }
            let left = bounds.origin.x + inset;
            let bottom = bounds.origin.y + bounds.size.height - inset;

            for length in [extent * 0.45, extent * 0.95] {
                let mut builder = PathBuilder::stroke(px(1.));
                builder.move_to(point(left, bottom - length));
                builder.line_to(point(left + length, bottom));
                match builder.build() {
                    Ok(path) => window.paint_path(path, color),
                    Err(error) => tracing::warn!("failed to build resize grip path: {error}"),
                }
            }
        },
    )
    .size_full()
}

/// The forked input propagates left/right at the text boundaries so a
/// navigable command palette can react to them. Inside a table cell that
/// would hand the keystroke on to the table's column selection, so the cell
/// editor swallows them instead.
fn swallow_boundary_movement<T: InteractiveElement>(element: T) -> T {
    element
        .on_action(|_: &MoveLeft, _, cx| cx.stop_propagation())
        .on_action(|_: &MoveRight, _, cx| cx.stop_propagation())
}

impl ResultsTableDelegate {
    /// Switches the cell being edited between quoted data and SQL the server
    /// evaluates. `key_context` is the editor it sits in, for the shortcut in
    /// its tooltip.
    fn render_expression_button(
        &self,
        key_context: &'static str,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        Button::new("cell-expression-toggle")
            .label("SQL")
            .ghost()
            .xsmall()
            .selected(self.edit_state.editing_as_expression)
            .tooltip_with_action(
                "Send as a SQL expression instead of a value",
                &ToggleCellExpression,
                Some(key_context),
            )
            .on_click(cx.listener(|table, _, _, cx| {
                Self::toggle_editing_expression(table, cx);
            }))
    }

    /// Expanded inline editor: an absolutely-positioned, larger input overlay
    /// with a minimize affordance.
    fn render_expanded_cell(
        &self,
        input: Entity<EditorState>,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_json = self.column_types.get(col_ix).copied() == Some(ColumnType::Json);
        let as_json = self.edit_state.expanded_as_json;
        let json_error = self.edit_state.json_error.clone();
        let overlay_size = self.edit_state.expanded_size.size();
        // The affordances sit over the editor's own text padding, so they line
        // up with the first line rather than crowding the border.
        let editor_size = Size::default();
        div()
            .bg(cx.theme().background)
            .border_2()
            .border_color(cx.theme().yellow)
            .p_0()
            .font_family(cx.theme().mono_font_family.clone())
            .child(
                gpui::deferred(
                    div()
                        .absolute()
                        .right(px(0.))
                        .top(px(0.))
                        .w(overlay_size.width)
                        .h(overlay_size.height)
                        .bg(cx.theme().background)
                        .shadow_lg()
                        // The table's scroll masks consume wheel events in the
                        // capture phase, before anything drawn above them gets
                        // a turn, and only defer to hitboxes that occlude them.
                        // Without this the table scrolls instead of the editor.
                        .occlude()
                        .key_context("ExpandedCellEditor")
                        .on_action(cx.listener(
                            move |table, _event: &gpui_component::input::Escape, window, cx| {
                                Self::handle_minimize(table, (col_ix, row_ix), is_json, window, cx);
                            },
                        ))
                        // The editor brings its own context menu. Without this
                        // the right click also reaches the table underneath,
                        // which opens the row menu on top of it.
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .map(swallow_boundary_movement)
                        // The editor only stops wheel events that moved its own
                        // offset, so at its scroll limits they would fall
                        // through and scroll the table underneath the overlay.
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(
                            v_flex()
                                .size_full()
                                .child(
                                    Editor::new(&input)
                                        .disabled(!self.is_editable())
                                        .flex_1()
                                        .min_h_0()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(12.)),
                                )
                                .when_some(json_error, |this, error| {
                                    this.child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .text_xs()
                                            .text_color(cx.theme().danger)
                                            .border_t_1()
                                            .border_color(cx.theme().border)
                                            .child(format!("Invalid JSON: {error}")),
                                    )
                                }),
                        )
                        // The editor takes no suffix, so the minimize
                        // affordance sits over its top-right corner.
                        .child(
                            h_flex()
                                .absolute()
                                .right(editor_size.input_px())
                                .top(editor_size.input_py())
                                .gap_1()
                                .when(as_json && self.is_editable(), |this| {
                                    this.child(
                                        Button::new((
                                            "expanded-json-format",
                                            row_ix * 1000 + col_ix,
                                        ))
                                        .label("Format")
                                        .ghost()
                                        .xsmall()
                                        .on_click(
                                            cx.listener(|table, _, window, cx| {
                                                Self::reflow_expanded_json(
                                                    table, false, window, cx,
                                                );
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new((
                                            "expanded-json-minify",
                                            row_ix * 1000 + col_ix,
                                        ))
                                        .label("Minify")
                                        .ghost()
                                        .xsmall()
                                        .on_click(
                                            cx.listener(|table, _, window, cx| {
                                                Self::reflow_expanded_json(table, true, window, cx);
                                            }),
                                        ),
                                    )
                                })
                                .child(self.render_expression_button("ExpandedCellEditor", cx))
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |table, _event, window, cx| {
                                                Self::handle_minimize(
                                                    table,
                                                    (col_ix, row_ix),
                                                    is_json,
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(Icon::new(IconName::Minimize).text_xs()),
                                ),
                        )
                        .child(self.render_expanded_resize_handle(row_ix, col_ix, cx)),
                )
                .with_priority(99),
            )
    }

    /// The overlay floats over the table rather than sharing a container
    /// with a sibling, so a resizable panel has nothing to resize against:
    /// this grip drags its size.
    fn render_expanded_resize_handle(
        &self,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .id(("expanded-cell-resize", row_ix * 1000 + col_ix))
            .absolute()
            .left(px(0.))
            .bottom(px(0.))
            .size(px(14.))
            .cursor(CursorStyle::ResizeUpRightDownLeft)
            .child(render_resize_grip(cx.theme().muted_foreground))
            .on_drag(ResizeExpandedCell, {
                let table_state = cx.entity();
                move |_, _, _, cx| {
                    table_state.update(cx, |table, _| {
                        table.delegate_mut().edit_state.expanded_size.start_drag()
                    });
                    cx.new(|_| ResizeExpandedCell)
                }
            })
            .on_drag_move(
                cx.listener(|table, event: &DragMoveEvent<ResizeExpandedCell>, _, cx| {
                    let position = event.event.position;
                    table
                        .delegate_mut()
                        .edit_state
                        .expanded_size
                        .drag_to(position);
                    cx.notify();
                }),
            )
    }

    /// A cell expanded into the binary viewer or the pair editor, floating
    /// over the grid like the expanded text editor.
    fn render_overlay_cell(
        &self,
        editor: AnyElement,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let overlay_size = self.edit_state.expanded_size.size();
        div()
            .size_full()
            .border_2()
            .border_color(cx.theme().yellow)
            .child(
                gpui::deferred(
                    div()
                        .absolute()
                        .right(px(0.))
                        .top(px(0.))
                        .w(overlay_size.width)
                        .h(overlay_size.height)
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .shadow_lg()
                        // As for the expanded text editor: without these the
                        // grid underneath scrolls, opens its row menu and
                        // moves its selection.
                        .occlude()
                        .key_context("CellOverlayEditor")
                        .on_action(cx.listener(
                            |table, _: &gpui_component::input::Escape, window, cx| {
                                Self::close_binary_viewer(table, window, cx);
                            },
                        ))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(editor)
                        .child(self.render_expanded_resize_handle(row_ix, col_ix, cx)),
                )
                .with_priority(99),
            )
    }

    /// Normal single-line inline editor with an expand (maximize) affordance
    /// and the bubble of actions on its value.
    fn render_inline_cell(
        &self,
        input: Entity<InputState>,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let column_type = self.column_types.get(col_ix).copied();
        let is_json = column_type == Some(ColumnType::Json);

        div()
            // Scopes the tab/shift-tab -> Edit{Next,Prev}Cell bindings to the
            // inline editor, so they outrank the table's own tab bindings.
            .key_context("CellEditor")
            .map(swallow_boundary_movement)
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .size_full()
            .flex()
            .items_center()
            .p_0()
            .when(column_type.is_some_and(|ct| ct.is_numeric()), |this| {
                this.justify_end()
            })
            .when(column_type == Some(ColumnType::Uuid), |this| {
                this.text_color(cx.theme().blue)
            })
            .when(column_type == Some(ColumnType::DateTime), |this| {
                this.text_color(cx.theme().green)
            })
            .when(column_type == Some(ColumnType::Json), |this| {
                this.text_color(cx.theme().yellow)
            })
            .when(column_type == Some(ColumnType::Array), |this| {
                this.text_color(cx.theme().blue)
            })
            .child(
                Input::new(&input)
                    .disabled(!self.is_editable())
                    // The cell is the frame here: an input border and the
                    // focus ring outside it would sit on top of the grid.
                    .bordered(false)
                    .flex_1()
                    .text_size(px(12.))
                    // The input sets 1.25rem, the static cell keeps GPUI's
                    // default. Their boxes snap to whole pixels but the text
                    // is placed by the unsnapped half leading, so with
                    // different line heights the text moves a pixel.
                    .line_height(gpui::phi())
                    .pl_0()
                    .suffix(
                        div()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |table, _event, window, cx| {
                                    Self::handle_maximize(
                                        table, row_ix, col_ix, is_json, window, cx,
                                    );
                                }),
                            )
                            .child(Icon::new(IconName::Maximize).text_xs()),
                    ),
            )
            .when(self.is_editable(), |this| {
                this.relative()
                    .child(self.render_cell_bubble(row_ix, col_ix, cx))
            })
    }

    /// The actions on the value being edited, in a bubble hanging off the
    /// cell's bottom right corner where they have room for a label. It is a
    /// sibling of the input rather than its suffix, so keys typed into the
    /// date picker do not reach the input's own bindings.
    fn render_cell_bubble(
        &self,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Below the last visible rows there is no grid left to draw over.
        let above = row_ix + 2 >= self.visible_rows.end && row_ix >= self.visible_rows.start + 2;
        div()
            .absolute()
            .right_0()
            .map(|this| {
                if above {
                    this.bottom(gpui::relative(1.))
                } else {
                    this.top(gpui::relative(1.))
                }
            })
            .child(
                gpui::deferred(
                    h_flex()
                        .my(px(2.))
                        .p(px(2.))
                        .gap(px(2.))
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .text_color(cx.theme().popover_foreground)
                        .font_family(cx.theme().font_family.clone())
                        .shadow_md()
                        // Drawn over the cells around it, which must not get
                        // its clicks or hovers.
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.render_expression_button("CellEditor", cx))
                        .when_some(self.temporal_shape(row_ix, col_ix), |this, shape| {
                            this.child(self.render_date_time_picker(shape, row_ix, col_ix, cx))
                        })
                        .when_some(self.enum_labels(col_ix), |this, labels| {
                            this.child(self.render_enum_picker(labels.to_vec(), row_ix, col_ix, cx))
                        }),
                )
                .with_priority(99),
            )
    }

    /// Calendar button of a date or time cell editor and the picker it opens.
    fn render_date_time_picker(
        &self,
        shape: TemporalShape,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let picker = self.edit_state.date_picker.clone();
        let table_state = cx.entity();
        Popover::new(("cell-date-time-picker", row_ix * 1000 + col_ix))
            .anchor(gpui::Anchor::TopRight)
            .open(picker.is_some())
            .on_open_change(move |open, window, cx| {
                table_state.update(cx, |state, cx| {
                    if *open {
                        Self::open_date_time_picker(state, shape, window, cx);
                    } else {
                        Self::close_date_time_picker(state, cx);
                    }
                });
            })
            .trigger(
                Button::new(("cell-date-time-picker-trigger", row_ix * 1000 + col_ix))
                    .icon(IconName::Calendar)
                    .label(match shape {
                        TemporalShape::Date => "Pick Date",
                        TemporalShape::Time => "Pick Time",
                        TemporalShape::Timestamp | TemporalShape::TimestampWithTimeZone => {
                            "Pick Date and Time"
                        }
                    })
                    .ghost()
                    .xsmall()
                    .disabled(!self.is_editable()),
            )
            .content(move |_, _, _| {
                div().when_some(picker.clone(), |this, picker| this.child(picker))
            })
    }

    /// Button of an enum cell editor and the list of the type's labels it
    /// opens. The label the editor holds is marked.
    fn render_enum_picker(
        &self,
        labels: Vec<String>,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let table_state = cx.entity();
        let current = self
            .edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
            .unwrap_or_default();
        Popover::new(("cell-enum-picker", row_ix * 1000 + col_ix))
            .anchor(gpui::Anchor::TopRight)
            .open(self.edit_state.enum_picker_open)
            .on_open_change({
                let table_state = table_state.clone();
                move |open, _window, cx| {
                    table_state.update(cx, |state, cx| {
                        Self::set_enum_picker_open(state, *open, cx);
                    });
                }
            })
            .trigger(
                Button::new(("cell-enum-picker-trigger", row_ix * 1000 + col_ix))
                    .icon(IconName::List)
                    .label("Pick Value")
                    .ghost()
                    .xsmall()
                    .disabled(!self.is_editable()),
            )
            .content(move |_, _, cx| {
                v_flex()
                    .id(("cell-enum-labels", row_ix * 1000 + col_ix))
                    .max_h(px(280.))
                    .min_w(px(160.))
                    .overflow_y_scroll()
                    .gap(px(1.))
                    .children(labels.iter().enumerate().map(|(index, label)| {
                        let table_state = table_state.clone();
                        let picked = label.clone();
                        Button::new(("cell-enum-label", index))
                            .label(label.clone())
                            .ghost()
                            .small()
                            .w_full()
                            .justify_start()
                            .selected(*label == current)
                            .on_click(move |_, window, cx| {
                                table_state.update(cx, |state, cx| {
                                    Self::pick_enum_label(
                                        state,
                                        (row_ix, col_ix),
                                        &picked,
                                        window,
                                        cx,
                                    );
                                });
                            })
                    }))
                    .text_color(cx.theme().popover_foreground)
            })
    }

    /// Static (non-editing) cell: display text plus hover affordances for
    /// clipboard copy and foreign-key lookup.
    fn render_static_cell(
        &self,
        row_ix: usize,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_edited = self.edit_state.is_edited(row_ix, col_ix);
        let is_editable = self.is_editable();
        let is_expression = self.edit_state.is_expression(row_ix, col_ix);

        let current_value: Option<String> = if is_edited {
            self.edit_state
                .get_edited_value(row_ix, col_ix)
                .and_then(|v| v.clone())
        } else {
            self.rows
                .get(row_ix)
                .and_then(|row| row.get(col_ix))
                .and_then(|v| v.clone())
        };

        let is_null = current_value.is_none();
        // Untouched cells of a pending new row show the server default that
        // will apply because the column is omitted from the INSERT.
        let uses_default = is_null && self.cell_uses_default(row_ix, col_ix);
        // The cell holds the start of a value left on the server.
        let deferred_size = self.deferred_size(row_ix, col_ix);
        let settings = results_settings(cx);
        let shown = match current_value.as_deref() {
            None if uses_default => DisplayText::from("DEFAULT".to_string()),
            None => DisplayText::from("NULL".to_string()),
            // Until it is applied, an expression is SQL text, not a value.
            Some(value) if is_expression => display_text(value, settings.show_invisible_characters),
            Some(value) => match deferred_size {
                Some(size) => deferred_display_text(
                    value,
                    size,
                    self.is_loading_deferred(row_ix),
                    settings.show_invisible_characters,
                ),
                None => self.display_value(col_ix, value, &settings),
            },
        };
        // The clipboard and the foreign key lookup get the stored value, not
        // the single-line rendering with its ⏎ / ⇥ markers or a reformatted
        // date.
        let stored_text = current_value.unwrap_or_else(|| shown.text.clone());

        // Check if this is a numeric column for right-alignment
        let is_numeric = self
            .column_types
            .get(col_ix)
            .is_some_and(|ct| ct.is_numeric());

        let cell_content = h_flex()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w_0()
            // Until it is applied, an expression shows as the SQL that was
            // typed, which would otherwise pass for a string holding that text.
            .when(is_expression, |this| {
                this.child(
                    Icon::new(IconName::SquareFunction)
                        .size(px(10.))
                        .text_color(cx.theme().muted_foreground),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(render_display_text(shown, cx)),
            )
            .child(
                div()
                    .invisible()
                    .group_hover("", |this| this.visible())
                    .map(|this| match deferred_size {
                        // The value is not here to hand to the clipboard:
                        // the panel fetches it first.
                        Some(_) => this.child(
                            Button::new(("cell-copy-deferred", row_ix * 1000 + col_ix))
                                .icon(IconName::Copy)
                                .ghost()
                                .xsmall()
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(CopyCellValue {
                                            row: row_ix,
                                            col: col_ix,
                                        }),
                                        cx,
                                    );
                                }),
                        ),
                        None => this.child(
                            Clipboard::new(format!("cell-clipboard-{}-{}", row_ix, col_ix))
                                .value(stored_text.clone()),
                        ),
                    }),
            )
            // By name, as the header does: the result's columns need not be
            // in the table's order.
            .when_some(
                self.table_column_info(col_ix)
                    .and_then(|c| c.foreign_key.as_ref()),
                |this, fk_info| {
                    let fk_info = fk_info.clone();
                    let cell_value = stored_text.clone();
                    let open_row = self.referenced_row_request(row_ix, col_ix);
                    let popover_id = format!("fk-popover-{}-{}", row_ix, col_ix);
                    let connection_id = self.connection_id;
                    let database_name = if self.database_name.is_empty() {
                        None
                    } else {
                        Some(self.database_name.clone())
                    };

                    this.child(
                        div()
                            .invisible()
                            .group_hover("", |this| this.visible())
                            .child(
                                Popover::new(popover_id.clone())
                                    .anchor(gpui::Anchor::BottomRight)
                                    .trigger(
                                        Button::new(format!("fk-trigger-{}", popover_id))
                                            .icon(IconName::Search)
                                            .ghost()
                                            .xsmall(),
                                    )
                                    .content(
                                        move |_state: &mut PopoverState,
                                              window: &mut Window,
                                              cx: &mut Context<PopoverState>| {
                                            let database_name_clone = database_name.clone();

                                            // Use use_keyed_state to lazily create the FK popover entity
                                            let fk_popover = window.use_keyed_state(
                                                popover_id.clone(),
                                                cx,
                                                |_id, cx| {
                                                    ForeignKeyPopover::new(
                                                        &fk_info.foreign_table_name,
                                                        &fk_info.foreign_column_name,
                                                        &cell_value,
                                                        connection_id,
                                                        database_name_clone,
                                                        open_row.clone(),
                                                        cx,
                                                    )
                                                },
                                            );
                                            div().max_w(px(300.)).child(fk_popover)
                                        },
                                    ),
                            ),
                    )
                },
            );

        let is_deleted = self.edit_state.is_row_deleted(row_ix);

        div()
            .group("")
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .size_full() // Fill the entire cell container
            .flex() // Enable flexbox layout
            .items_center() // Center vertically
            .when(is_numeric, |this| {
                this.text_align(gpui::TextAlign::Right)
                    .justify_end() // Right-align numeric columns
                    .text_color(cx.theme().foreground) // Ensure numeric text is visible
            })
            .when(
                self.column_types.get(col_ix) == Some(&ColumnType::Uuid),
                |this| {
                    this.text_color(cx.theme().blue) // Blue color for UUIDs
                },
            )
            .when(
                self.column_types.get(col_ix) == Some(&ColumnType::DateTime),
                |this| {
                    this.text_color(cx.theme().green) // Green color for timestamps
                },
            )
            .when(
                matches!(
                    self.column_types.get(col_ix),
                    Some(ColumnType::Json | ColumnType::KeyValue)
                ),
                |this| {
                    // Yellow for structured values: JSON and hstore.
                    this.text_color(cx.theme().yellow)
                },
            )
            .when(is_edited, |this| {
                this.bg(cx.theme().yellow.opacity(0.3))
                    .pl_2()
                    .border_l_2()
                    .border_color(cx.theme().yellow)
            })
            .when(deferred_size.is_some(), |this| {
                this.text_color(cx.theme().muted_foreground)
            })
            .when(is_null, |this| {
                this.text_color(cx.theme().muted_foreground).italic()
            })
            // Deleted rows are tinted red with a strikethrough across every cell,
            // since the row header no longer carries the per-row delete indicator.
            // Applied last so it overrides the column-type colors above.
            .when(is_deleted, |this| {
                this.line_through()
                    .text_color(cx.theme().red)
                    .bg(cx.theme().red.opacity(0.15))
            })
            // Only show visual feedback for editable cells when hovering
            .when(!is_null && is_editable, |this| this.cursor_pointer())
            .py_1()
            .child(cell_content)
    }
}

impl ResultsTableDelegate {
    /// Sort the rows by `keys`, the first key sorting first. An empty list
    /// leaves the order as it is.
    pub(crate) fn sort_rows(&mut self, keys: &[(usize, ColumnSort)]) {
        if keys.is_empty() {
            return;
        }
        let keys: Vec<_> = keys
            .iter()
            .map(|&(col_ix, sort)| {
                let is_numeric = self
                    .column_types
                    .get(col_ix)
                    .is_some_and(|col_type| col_type.is_numeric());
                (col_ix, sort, is_numeric)
            })
            .collect();

        let compare = |a: &Vec<Option<String>>, b: &Vec<Option<String>>| {
            keys.iter()
                .map(|&(col_ix, sort, is_numeric)| {
                    let a_val = a.get(col_ix).and_then(|s| s.as_deref()).unwrap_or("");
                    let b_val = b.get(col_ix).and_then(|s| s.as_deref()).unwrap_or("");
                    let ordering = if is_numeric {
                        compare_numeric(a_val, b_val)
                    } else {
                        a_val.cmp(b_val)
                    };
                    match sort {
                        ColumnSort::Descending => ordering.reverse(),
                        _ => ordering,
                    }
                })
                .find(|ordering| ordering.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        };

        self.rows.sort_by(compare);
        // Keep the hidden rows in the same order so clearing the filter does
        // not silently undo the sort.
        if let Some(unfiltered_rows) = self.unfiltered_rows.as_mut() {
            unfiltered_rows.sort_by(compare);
        }
    }
}

impl ResultsTableDelegate {
    /// The entries that hide and pin column `col`, shared by the header and
    /// cell menus.
    fn column_layout_items(&self, menu: PopupMenu, col: usize) -> PopupMenu {
        let can_rearrange = self.can_rearrange_columns();
        let is_pinned = col < self.pinned_column_count();
        menu.menu_with_icon_and_disabled(
            "Hide Column",
            Icon::new(IconName::EyeOff),
            Box::new(HideResultColumn { col }),
            !can_rearrange || self.visible_column_count() <= 1,
        )
        .menu_with_icon_and_disabled(
            if is_pinned {
                "Unpin Column"
            } else {
                "Pin Column"
            },
            Icon::new(if is_pinned {
                IconName::PinOff
            } else {
                IconName::Pin
            }),
            Box::new(PinResultColumn {
                col,
                pinned: !is_pinned,
            }),
            !can_rearrange,
        )
    }
}

impl TableDelegate for ResultsTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.visible_column_count()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns
            .get(col_ix)
            .cloned()
            .unwrap_or_else(|| Column::new(format!("col_{}", col_ix), format!("Column {}", col_ix)))
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.columns[col_ix];
        let col_info = self.table_column_info(col_ix);
        let has_fk = col_info.is_some_and(|c| c.foreign_key.is_some());
        let is_pk = self
            .table_columns
            .iter()
            .any(|c| c.is_primary_key && c.name == col.name);
        let is_nullable = col_info.is_some_and(|c| c.is_nullable);

        let tooltip_id = format!("col-tooltip-{}", col_ix);

        div()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .pt(px(1.))
            .id(tooltip_id)
            .when_some(col_info, |this, col_info| {
                // Clone all needed values before the closure
                let col_name = col.name.to_string();
                let data_type = col_info.data_type.clone();
                let is_nullable = col_info.is_nullable;
                let default_value = col_info.default_value.clone();
                let max_length = col_info.character_maximum_length;
                let comment = col_info.comment.clone();
                this.tooltip(move |window, cx| {
                    Tooltip::element({
                        let col_name = col_name.clone();
                        let data_type = data_type.clone();
                        let default_value = default_value.clone();
                        let comment = comment.clone();
                        move |_window, cx| {
                            v_flex()
                                .gap_1()
                                .child(div().font_weight(FontWeight::BOLD).child(col_name.clone()))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child("Type:")
                                        .child(data_type.clone())
                                        .when_some(max_length, |this, len| {
                                            this.child(format!("({})", len))
                                        }),
                                )
                                .child(h_flex().gap_2().child("Nullable:").child(if is_nullable {
                                    "Yes"
                                } else {
                                    "No"
                                }))
                                .when_some(default_value.clone(), |this, default| {
                                    this.child(h_flex().gap_2().child("Default:").child(default))
                                })
                                .when_some(comment.clone(), |this, comment| {
                                    this.child(
                                        div()
                                            .max_w(px(360.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(comment),
                                    )
                                })
                        }
                    })
                    .build(window, cx)
                })
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(col.name.to_string())
                    .when(is_pk, |this| {
                        this.child(
                            Icon::new(IconName::Key)
                                .size(px(10.))
                                .text_color(cx.theme().yellow),
                        )
                    })
                    .when(has_fk && !is_pk, |this| {
                        this.child(
                            Icon::new(IconName::Key)
                                .size(px(10.))
                                .text_color(cx.theme().blue),
                        )
                    })
                    .when(is_nullable && !is_pk, |this| {
                        this.child(
                            Icon::new(IconName::Asterisk)
                                .size(px(10.))
                                .text_color(cx.theme().green),
                        )
                    }),
            )
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_editing = self.edit_state.is_editing(row_ix, col_ix);

        if is_editing {
            let overlay_editor = self
                .edit_state
                .binary_viewer
                .clone()
                .map(IntoElement::into_any_element)
                .or_else(|| {
                    self.edit_state
                        .key_value_editor
                        .clone()
                        .map(IntoElement::into_any_element)
                });
            if let Some(editor) = overlay_editor {
                return self
                    .render_overlay_cell(editor, row_ix, col_ix, cx)
                    .into_any_element();
            }
            // Embed Input directly in the cell
            match self.edit_state.get_editing_input() {
                Some(CellInput::Expanded(input)) if self.edit_state.is_expanded(row_ix, col_ix) => {
                    self.render_expanded_cell(input, row_ix, col_ix, cx)
                        .into_any_element()
                }
                Some(CellInput::Inline(input)) => self
                    .render_inline_cell(input, row_ix, col_ix, cx)
                    .into_any_element(),
                _ => div().child("").into_any_element(),
            }
        } else {
            self.render_static_cell(row_ix, col_ix, cx)
                .into_any_element()
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.sort_rows(&[(col_ix, sort)]);
    }

    fn multi_sortable(&self) -> bool {
        true
    }

    fn perform_multi_sort(
        &mut self,
        keys: &[(usize, ColumnSort)],
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.sort_rows(keys);
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        // The table moves its own column state after this returns. When the
        // move is refused (edits pending, or across the pinned edge), rebuild
        // that state from the columns as they are.
        if !self.move_visible_column(col_ix, to_ix) {
            let state = cx.entity();
            cx.defer(move |cx| state.update(cx, |state, cx| state.refresh(cx)));
        }
    }

    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.visible_rows = visible_range;
    }

    fn visible_columns_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
    }

    fn render_last_empty_col(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Add extra space to ensure all columns are scrollable
        // This compensates for any viewport calculation issues
        div().w(px(30.0)).h_full().flex_shrink_0()
    }

    fn column_context_menu(
        &mut self,
        col_ix: usize,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        self.column_layout_items(
            column_copy_items(menu, col_ix, false, false).separator(),
            col_ix,
        )
    }

    fn context_menu(
        &mut self,
        cell: (usize, usize),
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        // Check if the column is nullable (cell.1 is the 0-based data column index)
        let is_nullable = self
            .table_column_info(cell.1)
            .is_some_and(|c| c.is_nullable);
        // "Set DEFAULT" only makes sense on a pending new row whose column has
        // a server default: it reverts the cell so the INSERT omits the column.
        let compare_kind = self.compare_selection_kind;
        let is_editable = self.is_editable();
        let can_filter = self.supports_server_filter();
        let can_set_default = self.edit_state.is_new_row(cell.0)
            && self
                .table_column_info(cell.1)
                .is_some_and(|info| info.default_value.is_some());

        let follows_foreign_key = self.referenced_row_request(cell.0, cell.1).is_some();
        menu.when(follows_foreign_key, |menu| {
            menu.menu_with_icon(
                "Open Referenced Row",
                Icon::new(IconName::ExternalLink),
                Box::new(OpenReferencedRow),
            )
            .separator()
        })
        .item(
            PopupMenuItem::submenu(
                "Copy as",
                PopupMenu::build(window, cx, |menu, _, _| {
                    menu.menu_with_icon("CSV", Icon::new(IconName::Sheet), Box::new(CopyAsCSV))
                        .menu_with_icon("TSV", Icon::new(IconName::Sheet), Box::new(CopyAsTSV))
                        .menu_with_icon("JSON", Icon::new(IconName::Braces), Box::new(CopyAsJSON))
                        .menu_with_icon("SQL", Icon::new(IconName::Database), Box::new(CopyAsSQL))
                        .menu_with_icon(
                            "VALUES",
                            Icon::new(IconName::Database),
                            Box::new(CopyAsVALUES),
                        )
                        .menu_with_icon(
                            "Markdown",
                            Icon::new(IconName::Markdown),
                            Box::new(CopyAsMarkdown),
                        )
                        .menu_with_icon("HTML", Icon::new(IconName::Code), Box::new(CopyAsHTML))
                }),
            )
            .icon(Icon::new(IconName::Copy)),
        )
        .item(
            PopupMenuItem::submenu(
                "Copy Column",
                PopupMenu::build(window, cx, move |menu, _, _| {
                    column_copy_items(menu, cell.1, true, true)
                }),
            )
            .icon(Icon::new(IconName::List)),
        )
        .item(
            PopupMenuItem::submenu(
                "Export as",
                PopupMenu::build(window, cx, |menu, _, _| {
                    menu.menu_with_icon("CSV", Icon::new(IconName::Sheet), Box::new(ExportAsCSV))
                        .menu_with_icon("TSV", Icon::new(IconName::Sheet), Box::new(ExportAsTSV))
                        .menu_with_icon("JSON", Icon::new(IconName::Braces), Box::new(ExportAsJSON))
                        .menu_with_icon("SQL", Icon::new(IconName::Database), Box::new(ExportAsSQL))
                        .menu_with_icon(
                            "Markdown",
                            Icon::new(IconName::Markdown),
                            Box::new(ExportAsMarkdown),
                        )
                        .menu_with_icon("HTML", Icon::new(IconName::Code), Box::new(ExportAsHTML))
                }),
            )
            .icon(Icon::new(IconName::Download)),
        )
        .separator()
        // Row operations
        .menu_with_icon_and_disabled(
            "Add Row",
            Icon::new(IconName::Plus),
            Box::new(AddRow),
            !is_editable,
        )
        .menu_with_icon(
            "Duplicate Row",
            Icon::new(IconName::Copy),
            Box::new(DuplicateRow { row: cell.0 }),
        )
        .menu_with_icon(
            "Delete Row",
            Icon::new(IconName::Delete),
            Box::new(DeleteRow { row: cell.0 }),
        )
        .when(is_nullable, |this| {
            this.menu_with_icon(
                "Set NULL",
                Icon::new(IconName::CircleX),
                Box::new(SetCellNull {
                    row: cell.0,
                    col: cell.1,
                }),
            )
        })
        .when(can_set_default, |this| {
            this.menu_with_icon(
                "Set DEFAULT",
                Icon::new(IconName::RotateCcw),
                Box::new(SetCellDefault {
                    row: cell.0,
                    col: cell.1,
                }),
            )
        })
        .when(can_filter, |this| {
            this.separator().menu_with_icon(
                "Filter by Value",
                Icon::new(IconName::Search),
                Box::new(FilterByCellValue {
                    row: cell.0,
                    col: cell.1,
                }),
            )
        })
        .separator()
        .map(|menu| self.column_layout_items(menu, cell.1))
        .separator()
        .menu_with_icon(
            "Select Cell for Comparison",
            Icon::new(IconName::GitCompare),
            Box::new(SelectCellForCompare {
                row: cell.0,
                col: cell.1,
            }),
        )
        .when(compare_kind == Some(CompareKind::Cell), |this| {
            this.menu_with_icon(
                "Compare Cell with Selected",
                Icon::new(IconName::GitCompare),
                Box::new(CompareCellWithSelected {
                    row: cell.0,
                    col: cell.1,
                }),
            )
        })
        .menu_with_icon(
            "Select Row for Comparison",
            Icon::new(IconName::GitCompare),
            Box::new(SelectRowForCompare { row: cell.0 }),
        )
        .when(compare_kind == Some(CompareKind::Row), |this| {
            this.menu_with_icon(
                "Compare Row with Selected",
                Icon::new(IconName::GitCompare),
                Box::new(CompareRowWithSelected { row: cell.0 }),
            )
        })
        .when(compare_kind.is_some(), |this| {
            this.menu_with_icon(
                "Clear Comparison Selection",
                Icon::new(IconName::CircleX),
                Box::new(ClearCompareSelection),
            )
        })
    }
}

/// The entries that copy one column's values. From the cell menu
/// (`selected_only`) they cover the selected rows when several are selected.
/// In a Copy Column submenu (`in_submenu`) the labels leave out what its
/// title says.
fn column_copy_items(
    menu: PopupMenu,
    column: usize,
    selected_only: bool,
    in_submenu: bool,
) -> PopupMenu {
    let label = |full: &'static str, short: &'static str| if in_submenu { short } else { full };
    let copy = |distinct: bool, as_in_list: bool| {
        Box::new(CopyColumnValues {
            column,
            distinct,
            as_in_list,
            selected_only,
        })
    };
    menu.menu_with_icon(
        label("Copy Column Values", "Values"),
        Icon::new(IconName::List),
        copy(false, false),
    )
    .menu_with_icon(
        label("Copy Distinct Column Values", "Distinct Values"),
        Icon::new(IconName::List),
        copy(true, false),
    )
    .menu_with_icon(
        label("Copy Column as IN List", "IN List"),
        Icon::new(IconName::Parentheses),
        copy(false, true),
    )
    .menu_with_icon(
        label("Copy Distinct Column as IN List", "Distinct IN List"),
        Icon::new(IconName::Parentheses),
        copy(true, true),
    )
}
