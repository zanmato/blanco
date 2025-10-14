use std::future::Future;
use anyhow::Result;
use gpui::{AppContext, Task};

pub struct Tokio {}

impl Tokio {
    /// Spawns the given future on a background thread, and returns it via a GPUI task
    /// This provides a Tokio-compatible runtime for async operations
    pub fn spawn_result<C, Fut, R>(cx: &C, f: Fut) -> Task<Result<R>>
    where
        C: AppContext,
        Fut: Future<Output = Result<R>> + Send + 'static,
        R: Send + 'static,
    {
        cx.background_spawn(f)
    }
}