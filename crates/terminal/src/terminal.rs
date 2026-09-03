//! A terminal emulator for GPUI built on the `alacritty_terminal` engine.
//!
//! [`Terminal`] owns the PTY, the emulator state and the reader thread that
//! feeds it. It exposes operations in terminal terms (resize to a grid, send a
//! keystroke, scroll, select) and produces a [`Frame`] snapshot the element in
//! `element.rs` paints. [`view::TerminalView`] wraps it into a focusable view
//! with keyboard handling, actions and a blinking cursor.

pub mod element;
mod keys;
mod mouse;
pub mod palette;
pub mod view;

#[cfg(test)]
mod terminal_test;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;

use alacritty_terminal::event::{Event as EngineEvent, EventListener, Notify as _, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode, point_to_viewport, viewport_to_point};
use alacritty_terminal::tty;
use anyhow::Context as _;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Hsla, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Task, TouchPhase,
    px,
};
use vte::ansi::{Color, CursorShape as EngineCursorShape, NamedColor};

use mouse::MouseReport;
pub use palette::TerminalPalette;

/// What to run and where.
#[derive(Debug, Clone)]
pub struct TerminalSpawn {
    /// Program to start; `None` runs the user's login shell.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    /// Extra environment on top of the inherited one.
    pub env: HashMap<String, String>,
    /// Lines of scrollback to keep.
    pub scrollback_lines: usize,
}

impl Default for TerminalSpawn {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            working_directory: None,
            env: HashMap::new(),
            scrollback_lines: 10_000,
        }
    }
}

/// The grid the terminal is laid out on, in cells and in the pixel size of a
/// cell. Rows and columns never drop below one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSize {
    pub rows: usize,
    pub columns: usize,
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl GridSize {
    pub fn new(rows: usize, columns: usize, cell_width: Pixels, line_height: Pixels) -> Self {
        Self {
            rows: rows.max(1),
            columns: columns.max(1),
            cell_width,
            line_height,
        }
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows.min(u16::MAX as usize) as u16,
            num_cols: self.columns.min(u16::MAX as usize) as u16,
            cell_width: f32::from(self.cell_width).round().max(1.0) as u16,
            cell_height: f32::from(self.line_height).round().max(1.0) as u16,
        }
    }
}

impl Default for GridSize {
    fn default() -> Self {
        Self::new(24, 80, px(8.0), px(16.0))
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// What the host needs to know about; grid changes arrive as `Wakeup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Wakeup,
    TitleChanged(Option<String>),
    Bell,
    /// The program asked for the cursor to blink (or stop blinking).
    BlinkChanged(bool),
    /// The child process is gone; `None` when its status is unknown.
    Exited(Option<ExitStatus>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

impl From<EngineCursorShape> for CursorShape {
    fn from(shape: EngineCursorShape) -> Self {
        match shape {
            EngineCursorShape::Block => Self::Block,
            EngineCursorShape::Underline => Self::Underline,
            EngineCursorShape::Beam => Self::Beam,
            EngineCursorShape::HollowBlock => Self::HollowBlock,
            EngineCursorShape::Hidden => Self::Hidden,
        }
    }
}

/// A run of adjacent cells on one row sharing a style. `text` may hold more
/// characters than `width` cells when zero-width combining marks follow a
/// base character, and fewer when a wide character spans two cells.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSpan {
    pub column: usize,
    pub width: usize,
    pub text: String,
    pub foreground: Hsla,
    /// `None` for the default background, which the element paints once.
    pub background: Option<Hsla>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameRow {
    pub row: usize,
    pub spans: Vec<FrameSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameCursor {
    pub row: usize,
    pub column: usize,
    pub shape: CursorShape,
    /// The character under the cursor, drawn inverted inside a block cursor.
    pub character: char,
    /// Whether that character occupies two cells.
    pub wide: bool,
}

/// A selection clipped to the viewport, in zero-based row/column cells with
/// inclusive ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSelection {
    pub start_row: usize,
    pub start_column: usize,
    pub end_row: usize,
    pub end_column: usize,
    pub is_block: bool,
}

/// Everything the element paints for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub size: GridSize,
    pub rows: Vec<FrameRow>,
    pub cursor: Option<FrameCursor>,
    pub selection: Option<FrameSelection>,
    pub background: Hsla,
}

/// Hands engine events to the GPUI side. The engine calls this from its reader
/// thread, so it only pushes onto a channel.
#[derive(Clone)]
struct Listener(smol::channel::Sender<EngineEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: EngineEvent) {
        if self.0.try_send(event).is_err() {
            // The terminal entity is gone; the reader thread winds down once
            // the PTY closes, nothing else to do with the event.
            tracing::trace!("terminal event dropped after the terminal was closed");
        }
    }
}

/// How pointer drags translate to a selection: which kind of selection a click
/// started and whether a drag is in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DragState {
    button: MouseButton,
    selecting: bool,
}

/// A PTY and emulator that exist but are not yet driven by an entity.
struct OpenedTerminal {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: GridSize,
    receiver: smol::channel::Receiver<EngineEvent>,
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: GridSize,
    palette: TerminalPalette,
    title: Option<String>,
    exited: bool,
    /// Sub-line wheel movement carried to the next event.
    scroll_remainder: Pixels,
    drag: Option<DragState>,
    /// Last cell a motion report was sent for, so motion is reported once per
    /// cell rather than once per pixel.
    last_motion_cell: Option<(usize, usize)>,
    _event_pump: Task<()>,
}

impl EventEmitter<TerminalEvent> for Terminal {}

impl Terminal {
    /// Start `spawn` on a fresh pseudo terminal sized to the default grid (the
    /// element resizes it on the first frame) and wrap it in an entity. Fails
    /// when no PTY can be opened or the program cannot be started.
    pub fn spawn(
        spawn: TerminalSpawn,
        window_id: u64,
        cx: &mut App,
    ) -> anyhow::Result<Entity<Self>> {
        let opened = Self::open(spawn, window_id)?;
        Ok(cx.new(|cx| Self::attach(opened, cx)))
    }

    fn open(spawn: TerminalSpawn, window_id: u64) -> anyhow::Result<OpenedTerminal> {
        let size = GridSize::default();
        let (sender, receiver) = smol::channel::unbounded();
        let config = Config {
            scrolling_history: spawn.scrollback_lines,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &size,
            Listener(sender.clone()),
        )));

        let mut env = spawn.env;
        env.entry("TERM".to_string())
            .or_insert_with(|| "xterm-256color".to_string());
        env.entry("COLORTERM".to_string())
            .or_insert_with(|| "truecolor".to_string());
        env.entry("TERM_PROGRAM".to_string())
            .or_insert_with(|| "blanco".to_string());
        if std::env::var_os("LANG").is_none() {
            env.entry("LANG".to_string())
                .or_insert_with(|| "en_US.UTF-8".to_string());
        }

        let options = tty::Options {
            shell: spawn
                .program
                .map(|program| tty::Shell::new(program, spawn.args)),
            working_directory: spawn.working_directory,
            drain_on_exit: true,
            env,
            #[cfg(unix)]
            child_signal_mask: None,
            #[cfg(windows)]
            escape_args: false,
        };
        let pty = tty::new(&options, size.window_size(), window_id)
            .context("opening a pseudo terminal")?;
        let event_loop = EventLoop::new(term.clone(), Listener(sender), pty, true, false)
            .context("starting the terminal reader")?;
        let notifier = Notifier(event_loop.channel());
        // The reader thread ends on its own when the PTY closes, so its join
        // handle is not kept.
        drop(event_loop.spawn());

        Ok(OpenedTerminal {
            term,
            notifier,
            size,
            receiver,
        })
    }

    fn attach(opened: OpenedTerminal, cx: &mut Context<Self>) -> Self {
        let receiver = opened.receiver;
        let event_pump = cx.spawn(async move |this, cx| {
            while let Ok(first) = receiver.recv().await {
                let mut batch = vec![first];
                while batch.len() < 128 {
                    match receiver.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                if this
                    .update(cx, |terminal, cx| terminal.handle_events(batch, cx))
                    .is_err()
                {
                    break;
                }
                smol::future::yield_now().await;
            }
        });

        Self {
            term: opened.term,
            notifier: opened.notifier,
            size: opened.size,
            palette: TerminalPalette::default(),
            title: None,
            exited: false,
            scroll_remainder: px(0.0),
            drag: None,
            last_motion_cell: None,
            _event_pump: event_pump,
        }
    }

    fn handle_events(&mut self, events: Vec<EngineEvent>, cx: &mut Context<Self>) {
        let mut woke = false;
        for event in events {
            match event {
                EngineEvent::Wakeup | EngineEvent::MouseCursorDirty => woke = true,
                EngineEvent::Title(title) => {
                    self.title = Some(title);
                    cx.emit(TerminalEvent::TitleChanged(self.title.clone()));
                }
                EngineEvent::ResetTitle => {
                    self.title = None;
                    cx.emit(TerminalEvent::TitleChanged(None));
                }
                EngineEvent::PtyWrite(text) => self.write(text.into_bytes()),
                EngineEvent::ClipboardStore(_, text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                EngineEvent::ClipboardLoad(_, format) => {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    self.write(format(&text).into_bytes());
                }
                EngineEvent::ColorRequest(index, format) => {
                    // Answer from the emulator's own table first: a program may
                    // have redefined the color with OSC 4 before asking.
                    let color = self.term.lock().colors()[index]
                        .unwrap_or_else(|| palette::hsla_to_rgb(self.palette.indexed(index)));
                    self.write(format(color).into_bytes());
                }
                EngineEvent::TextAreaSizeRequest(format) => {
                    self.write(format(self.size.window_size()).into_bytes());
                }
                EngineEvent::CursorBlinkingChange => {
                    let blinking = self.term.lock().cursor_style().blinking;
                    cx.emit(TerminalEvent::BlinkChanged(blinking));
                }
                EngineEvent::Bell => cx.emit(TerminalEvent::Bell),
                EngineEvent::Exit => self.exit(None, cx),
                EngineEvent::ChildExit(status) => self.exit(Some(status), cx),
            }
        }
        if woke {
            cx.emit(TerminalEvent::Wakeup);
            cx.notify();
        }
    }

    fn exit(&mut self, status: Option<ExitStatus>, cx: &mut Context<Self>) {
        if !self.exited {
            self.exited = true;
            cx.emit(TerminalEvent::Exited(status));
            cx.notify();
        }
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn palette(&self) -> &TerminalPalette {
        &self.palette
    }

    pub fn set_palette(&mut self, palette: TerminalPalette) {
        self.palette = palette;
    }

    /// The current mode flags (application cursor keys, mouse tracking, ...).
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Relayout the grid. Only a change in rows, columns or cell metrics is
    /// forwarded, so a pixel-level drag does not flood the child with SIGWINCH.
    pub fn resize(&mut self, size: GridSize) {
        if size == self.size {
            return;
        }
        let dimensions_changed = size.rows != self.size.rows || size.columns != self.size.columns;
        self.size = size;
        if dimensions_changed {
            self.term.lock().resize(size);
        }
        if let Err(error) = self.notifier.0.send(Msg::Resize(size.window_size())) {
            tracing::warn!("failed to resize the pseudo terminal: {error}");
        }
    }

    /// Raw bytes to the child's stdin.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    /// Typed input: scrolls back to the live screen and drops the selection,
    /// as a real terminal does when you type.
    fn input(&mut self, bytes: Vec<u8>) {
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
    }

    /// Send a keystroke; `false` when it is not terminal input and should go to
    /// the application instead.
    pub fn send_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        let mode = self.mode();
        match keys::encode(keystroke, mode) {
            Some(bytes) => {
                self.input(bytes);
                true
            }
            None => false,
        }
    }

    /// Text from the clipboard or an input method. Wrapped in bracketed-paste
    /// markers when the program asked for them, otherwise newlines become
    /// carriage returns like the Enter key.
    pub fn paste(&mut self, text: &str) {
        let bytes = if self.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes);
    }

    /// Tell programs that track focus (`CSI ? 1004 h`) about a focus change.
    pub fn focus_changed(&self, focused: bool) {
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.write(if focused {
                b"\x1b[I".as_slice()
            } else {
                b"\x1b[O".as_slice()
            });
        }
    }

    pub fn scroll_lines(&mut self, delta: i32) {
        self.term.lock().scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Forget the scrollback; the visible screen stays.
    pub fn clear_scrollback(&mut self) {
        let mut term = self.term.lock();
        term.grid_mut().clear_history();
        term.scroll_display(Scroll::Bottom);
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    pub fn clear_selection(&mut self) {
        self.term.lock().selection = None;
    }

    pub fn select_all(&mut self) {
        let mut term = self.term.lock();
        let start = GridPoint::new(term.topmost_line(), Column(0));
        let end = GridPoint::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
    }

    /// The whole buffer as text, scrollback included.
    pub fn text(&self) -> String {
        let term = self.term.lock();
        let start = GridPoint::new(term.topmost_line(), Column(0));
        let end = GridPoint::new(term.bottommost_line(), term.last_column());
        term.bounds_to_string(start, end)
    }

    /// Convert a position relative to the grid's origin into the cell under it
    /// (clamped to the grid) and which half of the cell was hit.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let column_position = (position.x / self.size.cell_width).max(0.0);
        let column = (column_position.floor() as usize).min(self.size.columns.saturating_sub(1));
        let row = ((position.y / self.size.line_height).max(0.0).floor() as usize)
            .min(self.size.rows.saturating_sub(1));
        let side = if column_position - (column as f32) < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        (column, row, side)
    }

    fn grid_point(term: &Term<Listener>, column: usize, row: usize) -> GridPoint {
        viewport_to_point(
            term.grid().display_offset(),
            GridPoint::<usize>::new(row, Column(column)),
        )
    }

    /// Mouse tracking wins over selection unless shift is held, the usual
    /// escape hatch for selecting inside a mouse-aware program.
    fn reports_mouse(&self, modifiers: &Modifiers) -> bool {
        self.mode().intersects(TermMode::MOUSE_MODE) && !modifiers.shift
    }

    /// `position` is relative to the grid's origin.
    pub fn mouse_down(&mut self, event: &MouseDownEvent, position: Point<Pixels>) {
        let (column, row, side) = self.cell_at(position);
        if self.reports_mouse(&event.modifiers) {
            if let Some(bytes) = mouse::encode(
                MouseReport::Press(event.button),
                column,
                row,
                &event.modifiers,
                self.mode(),
            ) {
                self.write(bytes);
            }
            self.drag = Some(DragState {
                button: event.button,
                selecting: false,
            });
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }
        let mut term = self.term.lock();
        let point = Self::grid_point(&term, column, row);
        if event.modifiers.shift && term.selection.is_some() {
            if let Some(selection) = term.selection.as_mut() {
                selection.update(point, side);
            }
        } else {
            let kind = match event.click_count {
                2 => SelectionType::Semantic,
                3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            term.selection = Some(Selection::new(kind, point, side));
        }
        self.drag = Some(DragState {
            button: MouseButton::Left,
            selecting: true,
        });
    }

    /// Pointer moved while a button is down. `position` is relative to the
    /// grid's origin; it may fall outside the grid during a drag.
    pub fn mouse_drag(&mut self, position: Point<Pixels>, modifiers: &Modifiers) {
        let Some(drag) = self.drag else {
            return;
        };
        let (column, row, side) = self.cell_at(position);
        if !drag.selecting {
            if self.last_motion_cell != Some((column, row))
                && let Some(bytes) = mouse::encode(
                    MouseReport::Drag(drag.button),
                    column,
                    row,
                    modifiers,
                    self.mode(),
                )
            {
                self.last_motion_cell = Some((column, row));
                self.write(bytes);
            }
            return;
        }

        let mut term = self.term.lock();
        let point = Self::grid_point(&term, column, row);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
        }
        // Dragging past the top or bottom edge scrolls the buffer along.
        let overflow_lines = if position.y < px(0.0) {
            ((-position.y / self.size.line_height).ceil() as i32).min(3)
        } else if position.y > self.size.line_height * self.size.rows as f32 {
            -(((position.y - self.size.line_height * self.size.rows as f32) / self.size.line_height)
                .ceil() as i32)
                .min(3)
        } else {
            0
        };
        if overflow_lines != 0 {
            term.scroll_display(Scroll::Delta(overflow_lines));
        }
    }

    /// Pointer moved with no button held; only mouse-aware programs in
    /// all-motion mode hear about it.
    pub fn mouse_moved(&mut self, position: Point<Pixels>, modifiers: &Modifiers) {
        if self.drag.is_some() || !self.reports_mouse(modifiers) {
            return;
        }
        let (column, row, _) = self.cell_at(position);
        if self.last_motion_cell == Some((column, row)) {
            return;
        }
        if let Some(bytes) = mouse::encode(MouseReport::Motion, column, row, modifiers, self.mode())
        {
            self.last_motion_cell = Some((column, row));
            self.write(bytes);
        }
    }

    pub fn mouse_up(&mut self, event: &MouseUpEvent, position: Point<Pixels>) {
        let drag = self.drag.take();
        self.last_motion_cell = None;
        if drag.is_some_and(|drag| !drag.selecting) {
            let (column, row, _) = self.cell_at(position);
            if let Some(bytes) = mouse::encode(
                MouseReport::Release(event.button),
                column,
                row,
                &event.modifiers,
                self.mode(),
            ) {
                self.write(bytes);
            }
            return;
        }
        // A click without a drag leaves no selection behind.
        let mut term = self.term.lock();
        if term
            .selection
            .as_ref()
            .is_some_and(|selection| selection.is_empty())
        {
            term.selection = None;
        }
    }

    /// Wheel movement: reported to mouse-aware programs, turned into arrow keys
    /// for full-screen programs that asked for alternate scrolling, otherwise
    /// scrolls the buffer. `position` is relative to the grid's origin.
    pub fn scroll_wheel(&mut self, event: &ScrollWheelEvent, position: Point<Pixels>) {
        let lines = match event.touch_phase {
            TouchPhase::Started => {
                self.scroll_remainder = px(0.0);
                return;
            }
            TouchPhase::Moved => {
                let line_height = self.size.line_height;
                let before = (self.scroll_remainder / line_height).trunc() as i32;
                self.scroll_remainder += event.delta.pixel_delta(line_height).y;
                let after = (self.scroll_remainder / line_height).trunc() as i32;
                // Reset at the edges so a change of direction responds at once.
                self.scroll_remainder %= line_height * self.size.rows as f32;
                after - before
            }
            TouchPhase::Ended | TouchPhase::Cancelled => return,
        };
        if lines == 0 {
            return;
        }

        let mode = self.mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !event.shift {
            let (column, row, _) = self.cell_at(position);
            let report = if lines > 0 {
                MouseReport::WheelUp
            } else {
                MouseReport::WheelDown
            };
            let modifiers = Modifiers {
                shift: event.shift,
                ..Modifiers::default()
            };
            for _ in 0..lines.unsigned_abs() {
                if let Some(bytes) = mouse::encode(report, column, row, &modifiers, mode) {
                    self.write(bytes);
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !event.shift {
            let arrow: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            let bytes = arrow.repeat(lines.unsigned_abs() as usize);
            self.write(bytes);
        } else {
            self.scroll_lines(lines);
        }
    }

    /// Snapshot the visible grid for painting.
    pub fn frame(&self) -> Frame {
        let term = self.term.lock();
        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let engine_colors = content.colors;
        let resolve = |color: Color| -> Hsla {
            let index = match color {
                Color::Named(named) => Some(palette::named_color_index(named)),
                Color::Indexed(index) => Some(index as usize),
                Color::Spec(_) => None,
            };
            match index.and_then(|index| engine_colors[index]) {
                Some(rgb) => palette::rgb_to_hsla(rgb),
                None => self.palette.resolve(color),
            }
        };

        let mut rows: Vec<FrameRow> = Vec::with_capacity(self.size.rows);
        for indexed in content.display_iter {
            let Some(viewport) = point_to_viewport(display_offset, indexed.point) else {
                continue;
            };
            let cell = indexed.cell;
            let row = viewport.line;
            let column = viewport.column.0;
            if rows.last().is_none_or(|last| last.row != row) {
                rows.push(FrameRow {
                    row,
                    spans: Vec::new(),
                });
            }
            let Some(current_row) = rows.last_mut() else {
                continue;
            };

            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                // The second cell of a wide character: widen the span that
                // holds the character instead of drawing anything.
                if let Some(span) = current_row.spans.last_mut()
                    && span.column + span.width == column
                {
                    span.width += 1;
                }
                continue;
            }

            let (mut foreground, mut background) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            let background = match background {
                Color::Named(NamedColor::Background) => None,
                other => Some(resolve(other)),
            };
            let mut foreground = resolve(foreground);
            if cell.flags.intersects(Flags::DIM) {
                foreground = foreground.opacity(0.66);
            }
            let hidden = cell.flags.contains(Flags::HIDDEN);
            let character = if hidden { ' ' } else { cell.c };
            let bold = cell.flags.intersects(Flags::BOLD);
            let italic = cell.flags.intersects(Flags::ITALIC);
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
            let strikethrough = cell.flags.intersects(Flags::STRIKEOUT);

            // A plain space on the default background draws nothing; ending
            // the span here keeps runs short and skips shaping blank tails.
            if character == ' ' && background.is_none() && !underline && !strikethrough {
                continue;
            }

            let extends_previous = current_row.spans.last().is_some_and(|span| {
                span.column + span.width == column
                    && span.foreground == foreground
                    && span.background == background
                    && span.bold == bold
                    && span.italic == italic
                    && span.underline == underline
                    && span.strikethrough == strikethrough
            });
            if extends_previous && let Some(span) = current_row.spans.last_mut() {
                span.text.push(character);
                span.width += 1;
                if let Some(extra) = cell.zerowidth() {
                    span.text.extend(extra.iter());
                }
                continue;
            }
            let mut text = String::new();
            text.push(character);
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra.iter());
            }
            current_row.spans.push(FrameSpan {
                column,
                width: 1,
                text,
                foreground,
                background,
                bold,
                italic,
                underline,
                strikethrough,
            });
        }

        let cursor = point_to_viewport(display_offset, content.cursor.point).map(|viewport| {
            let cell = &term.grid()[content.cursor.point];
            FrameCursor {
                row: viewport.line,
                column: viewport.column.0,
                shape: content.cursor.shape.into(),
                character: cell.c,
                wide: cell.flags.contains(Flags::WIDE_CHAR),
            }
        });

        let selection = content.selection.and_then(|range| {
            let rows = self.size.rows as i32;
            let start_row = range.start.line.0 + display_offset as i32;
            let end_row = range.end.line.0 + display_offset as i32;
            if end_row < 0 || start_row >= rows {
                return None;
            }
            let last_column = self.size.columns.saturating_sub(1);
            let clipped_start = start_row.max(0);
            let clipped_end = end_row.min(rows - 1);
            let (start_column, end_column) = if range.is_block {
                (range.start.column.0, range.end.column.0)
            } else {
                (
                    if clipped_start == start_row {
                        range.start.column.0
                    } else {
                        0
                    },
                    if clipped_end == end_row {
                        range.end.column.0
                    } else {
                        last_column
                    },
                )
            };
            Some(FrameSelection {
                start_row: clipped_start as usize,
                start_column: start_column.min(last_column),
                end_row: clipped_end as usize,
                end_column: end_column.min(last_column),
                is_block: range.is_block,
            })
        });

        Frame {
            size: self.size,
            rows,
            cursor,
            selection,
            background: self.palette.background,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Stops the reader thread, which hangs up on the child as the PTY
        // closes. A closed channel means the loop already ended.
        if let Err(error) = self.notifier.0.send(Msg::Shutdown) {
            tracing::debug!("terminal reader already stopped: {error}");
        }
    }
}

/// Convenience for hosts that want the grid line a selection or cursor refers
/// to in engine terms.
pub fn line_from_row(row: usize, display_offset: usize) -> Line {
    Line(row as i32 - display_offset as i32)
}
