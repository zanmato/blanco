use std::ops::Range;

use gpui::AppContext as _;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, CursorStyle, DragMoveEvent, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, PathBuilder, Render, StatefulInteractiveElement,
    Styled, Window, canvas, div, point, px,
};
use gpui_component::popover::{Popover, PopoverState};
use gpui_component::{
    ActiveTheme, Icon, Sizable, Size,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    h_flex,
    input::{Editor, EditorState, Input, InputState, MoveLeft, MoveRight},
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
    tooltip::Tooltip,
    v_flex,
};

use blanco_core::connection_trait::ColumnType;

use super::ResultsTableDelegate;
use crate::app::{
    AddRow, CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, CopyAsTSV, CopyAsVALUES, DeleteRow,
    DuplicateRow, ExportAsCSV, ExportAsJSON, ExportAsMarkdown, ExportAsSQL, ExportAsTSV,
    SetCellDefault, SetCellNull,
};
use crate::results_panel::cell_edit_state::{CellInput, compare_numeric, format_value_for_display};
use crate::results_panel::foreign_key_popover::ForeignKeyPopover;
use blanco_ui::IconName;

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
                            Editor::new(&input)
                                .disabled(!self.is_editable())
                                .size_full()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(12.)),
                        )
                        // The editor takes no suffix, so the minimize
                        // affordance sits over its top-right corner.
                        .child(
                            div()
                                .absolute()
                                .right(editor_size.input_px())
                                .top(editor_size.input_py())
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
                        )
                        // The overlay floats over the table rather than sharing
                        // a container with a sibling, so a resizable panel has
                        // nothing to resize against: this grip drags its size.
                        .child(
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
                                            table
                                                .delegate_mut()
                                                .edit_state
                                                .expanded_size
                                                .start_drag()
                                        });
                                        cx.new(|_| ResizeExpandedCell)
                                    }
                                })
                                .on_drag_move(cx.listener(
                                    |table, event: &DragMoveEvent<ResizeExpandedCell>, _, cx| {
                                        let position = event.event.position;
                                        table
                                            .delegate_mut()
                                            .edit_state
                                            .expanded_size
                                            .drag_to(position);
                                        cx.notify();
                                    },
                                )),
                        ),
                )
                .with_priority(99),
            )
    }

    /// Normal single-line inline editor with an expand (maximize) affordance.
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
        let display_text = if uses_default {
            "DEFAULT".to_string()
        } else if is_null {
            "NULL".to_string()
        } else {
            format_value_for_display(current_value.as_deref().unwrap_or(""))
        };

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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(display_text.clone()),
            )
            .child(
                div()
                    .invisible()
                    .group_hover("", |this| this.visible())
                    .child(
                        Clipboard::new(format!("cell-clipboard-{}-{}", row_ix, col_ix))
                            .value(display_text.clone()),
                    ),
            )
            .when_some(
                self.table_columns
                    .get(col_ix)
                    .and_then(|c| c.foreign_key.as_ref()),
                |this, fk_info| {
                    let fk_info = fk_info.clone();
                    let cell_value = display_text.clone();
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
                self.column_types.get(col_ix) == Some(&ColumnType::Json),
                |this| {
                    this.text_color(cx.theme().yellow) // Yellow color for JSON
                },
            )
            .when(is_edited, |this| {
                this.bg(cx.theme().yellow.opacity(0.3))
                    .pl_2()
                    .border_l_2()
                    .border_color(cx.theme().yellow)
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

impl TableDelegate for ResultsTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
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
        let col_info = self.table_columns.get(col_ix);
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
                this.tooltip(move |window, cx| {
                    Tooltip::element({
                        let col_name = col_name.clone();
                        let data_type = data_type.clone();
                        let default_value = default_value.clone();
                        move |_window, _cx| {
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
        // Get the column type
        let col_type = self
            .column_types
            .get(col_ix)
            .copied()
            .unwrap_or(ColumnType::Unknown);
        let is_numeric = col_type.is_numeric();

        let compare = |a: &Vec<Option<String>>, b: &Vec<Option<String>>| {
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
        };

        self.rows.sort_by(compare);
        // Keep the hidden rows in the same order so clearing the filter does
        // not silently undo the sort.
        if let Some(unfiltered_rows) = self.unfiltered_rows.as_mut() {
            unfiltered_rows.sort_by(compare);
        }
    }

    fn visible_rows_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
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

    fn context_menu(
        &mut self,
        cell: (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        // Check if the column is nullable (cell.1 is the 0-based data column index)
        let is_nullable = self
            .table_columns
            .get(cell.1)
            .is_some_and(|c| c.is_nullable);
        // "Set DEFAULT" only makes sense on a pending new row whose column has
        // a server default: it reverts the cell so the INSERT omits the column.
        let can_set_default = self.edit_state.is_new_row(cell.0)
            && self
                .table_column_info(cell.1)
                .is_some_and(|info| info.default_value.is_some());

        menu.menu_with_icon(
            "Copy as CSV",
            Icon::new(IconName::Sheet),
            Box::new(CopyAsCSV),
        )
        .menu_with_icon(
            "Copy as TSV",
            Icon::new(IconName::Sheet),
            Box::new(CopyAsTSV),
        )
        .menu_with_icon(
            "Copy as JSON",
            Icon::new(IconName::Braces),
            Box::new(CopyAsJSON),
        )
        .menu_with_icon(
            "Copy as SQL",
            Icon::new(IconName::Database),
            Box::new(CopyAsSQL),
        )
        .menu_with_icon(
            "Copy as VALUES",
            Icon::new(IconName::Database),
            Box::new(CopyAsVALUES),
        )
        .menu_with_icon(
            "Copy as Markdown",
            Icon::new(IconName::Markdown),
            Box::new(CopyAsMarkdown),
        )
        .separator()
        // Export operations
        .menu_with_icon(
            "Export as CSV",
            Icon::new(IconName::File),
            Box::new(ExportAsCSV),
        )
        .menu_with_icon(
            "Export as TSV",
            Icon::new(IconName::File),
            Box::new(ExportAsTSV),
        )
        .menu_with_icon(
            "Export as JSON",
            Icon::new(IconName::File),
            Box::new(ExportAsJSON),
        )
        .menu_with_icon(
            "Export as SQL",
            Icon::new(IconName::File),
            Box::new(ExportAsSQL),
        )
        .menu_with_icon(
            "Export as Markdown",
            Icon::new(IconName::File),
            Box::new(ExportAsMarkdown),
        )
        .separator()
        // Row operations
        .menu_with_icon("Add Row", Icon::new(IconName::Plus), Box::new(AddRow))
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
    }
}
