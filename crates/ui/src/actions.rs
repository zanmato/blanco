use gpui::{Action, actions};
use serde::Deserialize;

#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = bui, no_json)]
pub struct Confirm {
    /// Is confirm with secondary.
    pub secondary: bool,
}

actions!(bui, [Cancel, SelectUp, SelectDown, SelectLeft, SelectRight]);

// The table selects every row with this. gpui-component's actions have no
// select all, the table's other keys use them.
actions!(bui, [SelectAll]);
