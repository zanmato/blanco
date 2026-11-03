#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

pub(crate) mod actions;
pub mod diff_view;
pub mod draggable_tree;
pub mod graph_view;
#[cfg(test)]
mod i18n_test;
pub mod icon;
pub mod size_indicator;
pub mod sql_view;
pub mod sticky_rows;
pub mod tab;
pub mod table;
pub mod tree;

pub use diff_view::{DiffSource, DiffView};
pub use graph_view::{GraphEdge, GraphModel, GraphNode, GraphNodeField, GraphView};
pub use icon::IconName;
pub use size_indicator::SizeIndicator;
pub use sql_view::{SqlView, SqlViewMessage, highlight_each};
pub use tab::{Tab, TabBar, TabVariant};
