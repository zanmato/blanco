// Vendored from gpui-component (https://github.com/longbridge/gpui-component)
// Apache-2.0 licensed. See crates/ui/LICENSE-APACHE.
// Modified to group the TabBar overflow menu by an optional `Tab::group` key.

#[allow(clippy::module_inception)]
mod tab;
mod tab_bar;

pub use tab::*;
pub use tab_bar::*;
