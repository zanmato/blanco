//! A focusable view around a [`Terminal`]: keyboard input, clipboard actions,
//! a blinking cursor and input-method composition.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, Styled, Subscription, Window, actions, div,
};

use crate::element::{TerminalElement, TerminalStyle};
use crate::{Terminal, TerminalEvent};

actions!(
    terminal,
    [
        /// Copy the selected text.
        Copy,
        /// Paste the clipboard into the terminal.
        Paste,
        /// Select the whole buffer.
        SelectAll,
        /// Drop the scrollback, keeping the visible screen.
        ClearScrollback,
        /// Scroll back to the live screen.
        ScrollToBottom,
    ]
);

const BLINK_INTERVAL: Duration = Duration::from_millis(530);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalViewEvent {
    TitleChanged(Option<String>),
    Exited,
}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    style: Rc<dyn Fn(&App) -> TerminalStyle>,
    cursor_shown: bool,
    /// Bumped whenever blinking restarts; a stale blink task sees a newer
    /// epoch and stops.
    blink_epoch: usize,
    marked_text: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    /// `style` is consulted every frame so theme changes apply live.
    pub fn new(
        terminal: Entity<Terminal>,
        style: impl Fn(&App) -> TerminalStyle + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.subscribe(&terminal, |_, _, event, cx| match event {
                TerminalEvent::Wakeup | TerminalEvent::BlinkChanged(_) | TerminalEvent::Bell => {
                    cx.notify();
                }
                TerminalEvent::TitleChanged(title) => {
                    cx.emit(TerminalViewEvent::TitleChanged(title.clone()));
                }
                TerminalEvent::Exited(_) => cx.emit(TerminalViewEvent::Exited),
            }),
            cx.on_focus(&focus_handle, window, Self::focus_in),
            cx.on_blur(&focus_handle, window, Self::focus_out),
        ];
        Self {
            terminal,
            focus_handle,
            style: Rc::new(style),
            cursor_shown: true,
            blink_epoch: 0,
            marked_text: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    fn focus_in(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.read(cx).focus_changed(true);
        self.restart_blinking(cx);
    }

    fn focus_out(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.read(cx).focus_changed(false);
        self.blink_epoch += 1;
        self.cursor_shown = true;
        cx.notify();
    }

    /// Show the cursor now and start toggling it; typing calls this so the
    /// cursor is steady while keys are pressed.
    fn restart_blinking(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch += 1;
        self.cursor_shown = true;
        cx.notify();
        let epoch = self.blink_epoch;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.blink_epoch != epoch {
                            return false;
                        }
                        this.cursor_shown = !this.cursor_shown;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // While an input method composes, the platform delivers the text
        // through the input handler; raw keystrokes would double up.
        if self.marked_text.is_some() {
            return;
        }
        let handled = self
            .terminal
            .update(cx, |terminal, _| terminal.send_keystroke(&event.keystroke));
        if handled {
            self.restart_blinking(cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.read(cx).selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.terminal.update(cx, |terminal, cx| {
            terminal.paste(&text);
            cx.notify();
        });
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.select_all();
            cx.notify();
        });
    }

    fn clear_scrollback(&mut self, _: &ClearScrollback, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.clear_scrollback();
            cx.notify();
        });
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.scroll_to_bottom();
            cx.notify();
        });
    }

    /// Text an input method finished composing.
    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| {
            terminal.paste(text);
            cx.notify();
        });
        self.restart_blinking(cx);
    }

    pub(crate) fn set_marked_text(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        self.marked_text = text.filter(|text| !text.is_empty());
        cx.notify();
    }

    pub(crate) fn marked_text_range(&self) -> Option<std::ops::Range<usize>> {
        self.marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let style = (self.style)(cx);
        div()
            .id("terminal-view")
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_scrollback))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(TerminalElement::new(
                self.terminal.clone(),
                cx.entity().downgrade(),
                self.focus_handle.clone(),
                focused,
                !focused || self.cursor_shown,
                style,
                self.marked_text.clone(),
            ))
    }
}
