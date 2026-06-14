//! App-wide activity reporting feeding the bottom status bar.
//!
//! Any context (foreground GPUI tasks or background tokio tasks that have no
//! `cx`) reports work by calling [`ActivityReporter::begin`], which returns an
//! [`ActivityGuard`]. The guard sends a `Begin` message immediately and an `End`
//! message when finished or dropped, so even error paths that bail out with `?`
//! clear the line. A single listener task in `app.rs` folds these messages into
//! the [`StatusBarState`] entity that the bar renders.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::{Context, Global, SharedString, Task};
use smol::channel;

/// How long a finished activity's outcome lingers before the bar returns to idle.
const TRANSIENT_DURATION: Duration = Duration::from_secs(3);

/// The outcome of a finished activity, flashed briefly in the status bar.
#[derive(Clone, Debug)]
pub enum ActivityResult {
    Ok(SharedString),
    Err(SharedString),
}

/// Messages sent over the activity channel to the listener task in `app.rs`.
#[derive(Debug)]
pub enum ActivityMessage {
    Begin {
        id: u64,
        label: SharedString,
    },
    End {
        id: u64,
        result: Option<ActivityResult>,
    },
}

/// Best-effort send: a full/closed channel only happens during shutdown, where
/// dropping a status update is harmless, so we trace rather than propagate.
fn send(sender: &channel::Sender<ActivityMessage>, message: ActivityMessage) {
    if let Err(err) = sender.try_send(message) {
        tracing::trace!("dropping activity status message: {err}");
    }
}

/// Global handle used to report activity from anywhere. Cheap to clone, so
/// background tasks can capture a clone before `move`.
#[derive(Clone)]
pub struct ActivityReporter {
    sender: channel::Sender<ActivityMessage>,
    next_id: Arc<AtomicU64>,
}

impl Global for ActivityReporter {}

impl ActivityReporter {
    pub fn new(sender: channel::Sender<ActivityMessage>) -> Self {
        Self {
            sender,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Fetch a clone of the global reporter.
    pub fn global(cx: &gpui::App) -> Self {
        cx.global::<Self>().clone()
    }

    /// Begin reporting an activity. The returned guard keeps it shown until it
    /// is dropped or [`ActivityGuard::finish`]ed.
    pub fn begin(&self, label: impl Into<SharedString>) -> ActivityGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        send(
            &self.sender,
            ActivityMessage::Begin {
                id,
                label: label.into(),
            },
        );
        ActivityGuard {
            sender: self.sender.clone(),
            id,
            finished: false,
        }
    }
}

/// RAII handle for one in-flight activity. Dropping it clears the activity;
/// calling [`finish`](Self::finish) instead flashes an outcome first.
pub struct ActivityGuard {
    sender: channel::Sender<ActivityMessage>,
    id: u64,
    finished: bool,
}

impl ActivityGuard {
    /// Mark the activity done and flash an outcome (✓ / ✗) for a few seconds.
    pub fn finish(mut self, result: ActivityResult) {
        send(
            &self.sender,
            ActivityMessage::End {
                id: self.id,
                result: Some(result),
            },
        );
        self.finished = true;
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if !self.finished {
            send(
                &self.sender,
                ActivityMessage::End {
                    id: self.id,
                    result: None,
                },
            );
        }
    }
}

/// Visual category for the current status line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    Busy,
    Ok,
    Err,
    Idle,
}

/// A resolved one-line summary of current activity, ready to render.
pub struct StatusLine {
    pub kind: StatusKind,
    pub text: SharedString,
}

/// Entity holding live activity state. Mutated only by the listener task in
/// `app.rs`; the app observes it and re-renders the status bar.
pub struct StatusBarState {
    active: Vec<(u64, SharedString)>,
    transient: Option<(ActivityResult, Instant)>,
    _clear_task: Task<()>,
}

impl Default for StatusBarState {
    fn default() -> Self {
        Self {
            active: Vec::new(),
            transient: None,
            _clear_task: Task::ready(()),
        }
    }
}

impl StatusBarState {
    /// Apply one channel message, scheduling the transient-clear timer as needed.
    pub fn apply(&mut self, message: ActivityMessage, cx: &mut Context<Self>) {
        match message {
            ActivityMessage::Begin { id, label } => {
                self.active.push((id, label));
            }
            ActivityMessage::End { id, result } => {
                self.active.retain(|(active_id, _)| *active_id != id);
                if let Some(result) = result {
                    self.set_transient(result, cx);
                }
            }
        }
        cx.notify();
    }

    /// Flash a standalone outcome (not tied to a tracked activity), e.g. a
    /// connection coming up or going down.
    pub fn flash(&mut self, result: ActivityResult, cx: &mut Context<Self>) {
        self.set_transient(result, cx);
        cx.notify();
    }

    fn set_transient(&mut self, result: ActivityResult, cx: &mut Context<Self>) {
        self.transient = Some((result, Instant::now()));
        // Replacing the task cancels any earlier pending clear, so when this
        // timer fires it is the most recent outcome.
        self._clear_task = cx.spawn(async move |handle, cx| {
            cx.background_executor().timer(TRANSIENT_DURATION).await;
            handle
                .update(cx, |state, cx| {
                    if state.transient.is_some() {
                        state.transient = None;
                        cx.notify();
                    }
                })
                .ok();
        });
    }

    /// Resolve what the bar should show right now.
    pub fn display(&self) -> StatusLine {
        if let Some((_, label)) = self.active.last() {
            let text = if self.active.len() > 1 {
                format!("{label} (+{})", self.active.len() - 1).into()
            } else {
                label.clone()
            };
            return StatusLine {
                kind: StatusKind::Busy,
                text,
            };
        }

        if let Some((result, started)) = &self.transient {
            if started.elapsed() < TRANSIENT_DURATION {
                return match result {
                    ActivityResult::Ok(text) => StatusLine {
                        kind: StatusKind::Ok,
                        text: text.clone(),
                    },
                    ActivityResult::Err(text) => StatusLine {
                        kind: StatusKind::Err,
                        text: text.clone(),
                    },
                };
            }
        }

        StatusLine {
            kind: StatusKind::Idle,
            text: "Ready".into(),
        }
    }
}
