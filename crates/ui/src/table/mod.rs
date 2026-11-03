//! The data table behind blanco's grids, vendored from gpui-component.
//!
//! Blanco owns this copy so its selection model, multi sort and column
//! fitting can grow without carrying a fork of the upstream table. It still
//! builds on gpui-component's menus, scrollbars and theme.

use gpui::App;

mod column;
mod data_table;
mod delegate;
mod loading;
mod selection;
mod state;

pub use column::*;
pub use data_table::*;
pub use delegate::*;
pub use selection::*;
pub use state::*;

/// Registers the table's key bindings. Call once at startup.
pub fn init(cx: &mut App) {
    data_table::init(cx);
}

/// Whether render timings are logged, read once per process.
fn measure_enable() -> bool {
    static MEASURE_ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(gpui_base::measurement_enabled);
    *MEASURE_ENABLED
}

#[cfg(test)]
pub(crate) fn test_init(cx: &mut App) {
    gpui_component::init(cx);
    init(cx);
}
