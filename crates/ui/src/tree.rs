use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    App, Context, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding,
    ListSizingBehavior, MouseButton, MouseDownEvent, ParentElement, Render, RenderOnce,
    SharedString, StyleRefinement, Styled, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, px, uniform_list,
};

use gpui_component::{
    StyledExt,
    list::ListItem,
    menu::{ContextMenuExt, PopupMenu},
    scroll::Scrollbar,
};

use crate::actions::{Confirm, SelectDown, SelectLeft, SelectRight, SelectUp};

const CONTEXT: &str = "BroTree";
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
    ]);
}

/// A delegate trait for providing tree data and rendering.
pub trait TreeDelegate: Sized + 'static {
    /// The metadata type associated with each tree item.
    type Metadata: Clone + 'static;

    /// Render the tree item at the given index.
    fn render_item(
        &self,
        ix: usize,
        entry: &TreeEntry<Self::Metadata>,
        selected: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> ListItem;

    /// Render the context menu for the tree item at the given index.
    /// `selection` holds every selected entry that is showing, the clicked one
    /// included: right-clicking outside the selection selects that item alone.
    fn context_menu(
        &self,
        _ix: usize,
        _entry: &TreeEntry<Self::Metadata>,
        _selection: &[&TreeEntry<Self::Metadata>],
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut App,
    ) -> PopupMenu {
        menu
    }
}

struct TreeItemState {
    expanded: bool,
    disabled: bool,
}

/// A tree item with a label, children, metadata, and an expanded state.
#[derive(Clone)]
pub struct TreeItem<M> {
    pub id: SharedString,
    pub label: SharedString,
    /// Shared so cloning a `TreeItem` (which happens for every entry on each
    /// rebuild) does not deep-copy the subtree.
    pub children: Rc<Vec<TreeItem<M>>>,
    pub metadata: M,
    state: Rc<RefCell<TreeItemState>>,
}

/// A flat representation of a tree item with its depth.
#[derive(Clone)]
pub struct TreeEntry<M> {
    pub item: TreeItem<M>,
    depth: usize,
}

impl<M> TreeEntry<M> {
    /// Get the source tree item.
    #[inline]
    pub fn item(&self) -> &TreeItem<M> {
        &self.item
    }

    /// The depth of this item in the tree.
    #[inline]
    pub fn depth(&self) -> usize {
        self.depth
    }

    #[inline]
    fn is_root(&self) -> bool {
        self.depth == 0
    }

    /// Whether this item is a folder (has children).
    #[inline]
    pub fn is_folder(&self) -> bool {
        self.item.is_folder()
    }

    /// Return true if the item is expanded.
    #[inline]
    pub fn is_expanded(&self) -> bool {
        self.item.is_expanded()
    }

    #[inline]
    pub fn is_disabled(&self) -> bool {
        self.item.is_disabled()
    }
}

impl<M> TreeItem<M> {
    /// Create a new tree item with the given label and metadata.
    ///
    /// - The `id` for you to uniquely identify this item, then later you can use it for selection or other purposes.
    /// - The `label` is the text to display for this item.
    /// - The `metadata` is custom data associated with this item.
    ///
    /// For example, the `id` is the full file path, the `label` is the file name,
    /// and `metadata` could contain file information.
    ///
    /// ```ignore
    /// TreeItem::new("src/ui/button.rs", "button.rs", FileMetadata { size: 1024 })
    /// ```
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, metadata: M) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            children: Rc::new(Vec::new()),
            metadata,
            state: Rc::new(RefCell::new(TreeItemState {
                expanded: false,
                disabled: false,
            })),
        }
    }

    /// Set expanded state for this tree item.
    pub fn expanded(self, expanded: bool) -> Self {
        self.state.borrow_mut().expanded = expanded;
        self
    }

    /// Set disabled state for this tree item.
    pub fn disabled(self, disabled: bool) -> Self {
        self.state.borrow_mut().disabled = disabled;
        self
    }

    /// Whether this item is a folder (has children).
    #[inline]
    pub fn is_folder(&self) -> bool {
        !self.children.is_empty()
    }

    /// Return true if the item is disabled.
    pub fn is_disabled(&self) -> bool {
        self.state.borrow().disabled
    }

    /// Return true if the item is expanded.
    #[inline]
    pub fn is_expanded(&self) -> bool {
        self.state.borrow().expanded
    }
}

impl<M: Clone> TreeItem<M> {
    /// Add a child item to this tree item.
    pub fn child(mut self, child: TreeItem<M>) -> Self {
        Rc::make_mut(&mut self.children).push(child);
        self
    }

    /// Add multiple child items to this tree item.
    pub fn children(mut self, children: impl IntoIterator<Item = TreeItem<M>>) -> Self {
        Rc::make_mut(&mut self.children).extend(children);
        self
    }
}

/// State for managing tree items.
pub struct TreeState<D: TreeDelegate> {
    focus_handle: FocusHandle,
    entries: Vec<TreeEntry<D::Metadata>>,
    scroll_handle: UniformListScrollHandle,
    selected_ix: Option<usize>,
    /// The id of the selected item. The index moves when the entries are
    /// rebuilt, so the selection is kept by id and looked up again; an item
    /// hidden by a collapsed parent is selected again once it shows.
    selected_id: Option<SharedString>,
    /// Items selected along with the selected one by secondary- or
    /// Shift-clicking, kept by id like it.
    extra_selected: Vec<SharedString>,
    right_clicked_index: Option<usize>,
    delegate: D,
}

impl<D: TreeDelegate> TreeState<D> {
    /// Create a new empty tree state.
    pub fn new(delegate: D, cx: &mut App) -> Self {
        Self {
            selected_ix: None,
            selected_id: None,
            extra_selected: Vec::new(),
            right_clicked_index: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::default(),
            entries: Vec::new(),
            delegate,
        }
    }

    pub fn entries(&self) -> &Vec<TreeEntry<D::Metadata>> {
        &self.entries
    }

    /// Get mutable reference to all entries
    pub fn entries_mut(&mut self) -> &mut Vec<TreeEntry<D::Metadata>> {
        &mut self.entries
    }

    /// Get mutable reference to entry at specific index
    pub fn get_mut(&mut self, ix: usize) -> Option<&mut TreeEntry<D::Metadata>> {
        self.entries.get_mut(ix)
    }

    /// Update an entry at specific index and notify the UI
    pub fn update_entry(
        &mut self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<&mut TreeEntry<D::Metadata>> {
        let entry = self.entries.get_mut(ix);
        if entry.is_some() {
            cx.notify();
        }
        entry
    }

    /// Find entry index by ID
    pub fn find_index(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.item.id == id)
    }

    /// Find entry by ID and get mutable reference
    pub fn find_mut(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<&mut TreeEntry<D::Metadata>> {
        if let Some(ix) = self.find_index(id) {
            self.update_entry(ix, cx)
        } else {
            None
        }
    }

    /// Set the tree items.
    pub fn set_items(
        &mut self,
        items: impl Into<Vec<TreeItem<D::Metadata>>>,
        cx: &mut Context<Self>,
    ) {
        let items = items.into();
        self.entries.clear();
        for item in items.into_iter() {
            self.add_entry(item, 0);
        }
        self.reselect();
        cx.notify();
    }

    /// Get the currently selected index, if any.
    pub fn selected_index(&self) -> Option<usize> {
        self.selected_ix
    }

    /// Set the selected index, or `None` to clear selection.
    pub fn set_selected_index(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.select(ix);
        cx.notify();
    }

    /// Select the item with `id`, which may be hidden under a collapsed parent
    /// for now. Returns its index when it is showing.
    pub fn select_id(
        &mut self,
        id: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.selected_id = Some(id.into());
        self.reselect();
        cx.notify();
        self.selected_ix
    }

    /// Whether the entry at `ix` is selected, alone or with others.
    pub fn is_selected(&self, ix: usize) -> bool {
        Some(ix) == self.selected_ix
            || self
                .entries
                .get(ix)
                .is_some_and(|entry| self.extra_selected.contains(&entry.item.id))
    }

    /// Every selected entry that is showing, in tree order.
    pub fn selected_entries(&self) -> Vec<&TreeEntry<D::Metadata>> {
        (0..self.entries.len())
            .filter(|&ix| self.is_selected(ix))
            .map(|ix| &self.entries[ix])
            .collect()
    }

    /// Add the entry at `ix` to the selection, or take it out.
    fn toggle_selected(&mut self, ix: usize) {
        let Some(id) = self.entries.get(ix).map(|entry| entry.item.id.clone()) else {
            return;
        };
        if self.selected_id.as_ref() == Some(&id) {
            self.selected_id = self.extra_selected.pop();
            self.reselect();
        } else if let Some(position) = self.extra_selected.iter().position(|other| *other == id) {
            self.extra_selected.remove(position);
        } else {
            self.extra_selected.extend(self.selected_id.take());
            self.selected_id = Some(id);
            self.selected_ix = Some(ix);
        }
    }

    /// Select every entry from the selected one to `ix`.
    fn select_range(&mut self, ix: usize) {
        let Some(anchor) = self.selected_ix else {
            self.select(Some(ix));
            return;
        };
        let range = anchor.min(ix)..=anchor.max(ix);
        self.extra_selected = range
            .filter(|&other| other != ix)
            .filter_map(|other| self.entries.get(other))
            .map(|entry| entry.item.id.clone())
            .collect();
        self.selected_ix = Some(ix);
        self.selected_id = self.entries.get(ix).map(|entry| entry.item.id.clone());
    }

    fn select(&mut self, ix: Option<usize>) {
        self.extra_selected.clear();
        self.selected_ix = ix;
        self.selected_id = ix
            .and_then(|ix| self.entries.get(ix))
            .map(|e| e.item.id.clone());
    }

    /// Point the selected index at the selected item again after the entries
    /// changed.
    fn reselect(&mut self) {
        self.selected_ix = self
            .selected_id
            .as_ref()
            .and_then(|id| self.entries.iter().position(|e| &e.item.id == id));
    }

    pub fn scroll_to_item(&mut self, ix: usize, strategy: gpui::ScrollStrategy) {
        self.scroll_handle.scroll_to_item(ix, strategy);
    }

    /// Get the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&TreeEntry<D::Metadata>> {
        self.selected_ix.and_then(|ix| self.entries.get(ix))
    }

    /// Get the delegate.
    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    fn add_entry(&mut self, item: TreeItem<D::Metadata>, depth: usize) {
        self.entries.push(TreeEntry {
            item: item.clone(),
            depth,
        });
        if item.is_expanded() {
            for child in item.children.iter() {
                self.add_entry(child.clone(), depth + 1);
            }
        }
    }

    fn toggle_expand(&mut self, ix: usize) {
        let Some(entry) = self.entries.get_mut(ix) else {
            return;
        };
        if !entry.is_folder() {
            return;
        }

        entry.item.state.borrow_mut().expanded = !entry.is_expanded();
        self.rebuild_entries();
        self.reselect();
    }

    fn rebuild_entries(&mut self) {
        let root_items: Vec<TreeItem<D::Metadata>> = self
            .entries
            .iter()
            .filter(|e| e.is_root())
            .map(|e| e.item.clone())
            .collect();
        self.entries.clear();
        for item in root_items.into_iter() {
            self.add_entry(item, 0);
        }
    }

    fn on_action_confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected_ix) = self.selected_ix
            && let Some(entry) = self.entries.get(selected_ix)
            && entry.is_folder()
        {
            self.toggle_expand(selected_ix);
            cx.notify();
        }
    }

    fn on_action_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected_ix) = self.selected_ix
            && let Some(entry) = self.entries.get(selected_ix)
            && entry.is_folder()
            && entry.is_expanded()
        {
            self.toggle_expand(selected_ix);
            cx.notify();
        }
    }

    fn on_action_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected_ix) = self.selected_ix
            && let Some(entry) = self.entries.get(selected_ix)
            && entry.is_folder()
            && !entry.is_expanded()
        {
            self.toggle_expand(selected_ix);
            cx.notify();
        }
    }

    fn on_action_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let mut selected_ix = self.selected_ix.unwrap_or(0);

        if selected_ix > 0 {
            selected_ix -= 1;
        } else {
            selected_ix = self.entries.len().saturating_sub(1);
        }

        self.select(Some(selected_ix));
        self.scroll_handle
            .scroll_to_item(selected_ix, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn on_action_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let mut selected_ix = self.selected_ix.unwrap_or(0);
        if selected_ix + 1 < self.entries.len() {
            selected_ix += 1;
        } else {
            selected_ix = 0;
        }

        self.select(Some(selected_ix));
        self.scroll_handle
            .scroll_to_item(selected_ix, gpui::ScrollStrategy::Bottom);
        cx.notify();
    }

    fn on_entry_click(
        &mut self,
        ix: usize,
        modifiers: gpui::Modifiers,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.secondary() {
            self.toggle_selected(ix);
        } else if modifiers.shift {
            self.select_range(ix);
        } else {
            self.select(Some(ix));
            self.toggle_expand(ix);
        }
        cx.notify();
    }
}

impl<D: TreeDelegate> Render for TreeState<D> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tree-state")
            .size_full()
            .relative()
            .context_menu({
                let view = cx.entity();
                move |this, window: &mut Window, cx: &mut Context<PopupMenu>| {
                    if let Some(ix) = view.read(cx).right_clicked_index {
                        view.update(cx, |state, cx| {
                            // `right_clicked_index` may be stale if `entries`
                            // changed between the right-click and this menu
                            // build, so guard the lookup rather than indexing.
                            let Some(entry) = state.entries.get(ix) else {
                                return this;
                            };
                            let selection = state.selected_entries();
                            state
                                .delegate()
                                .context_menu(ix, entry, &selection, this, window, cx)
                        })
                    } else {
                        this
                    }
                }
            })
            .child(
                uniform_list("entries", self.entries.len(), {
                    cx.processor(move |state, visible_range: Range<usize>, window, cx| {
                        let mut items = Vec::with_capacity(visible_range.len());
                        for ix in visible_range {
                            let entry = &state.entries[ix];
                            let selected = state.is_selected(ix);

                            let item = state.delegate.render_item(ix, entry, selected, window, cx);

                            let el = div()
                                .id(ix)
                                .child(item.disabled(entry.item().is_disabled()).selected(selected))
                                .when(!entry.item().is_disabled(), |this| {
                                    this.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener({
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                this.on_entry_click(
                                                    ix,
                                                    event.modifiers,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener({
                                            move |this, _, _window, cx| {
                                                if !this.is_selected(ix) {
                                                    this.select(Some(ix));
                                                }
                                                this.right_clicked_index = Some(ix);
                                                cx.notify();
                                            }
                                        }),
                                    )
                                });

                            items.push(el)
                        }

                        items
                    })
                })
                .flex_grow(1.)
                .size_full()
                .track_scroll(&self.scroll_handle)
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .into_any_element(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(4. * 2. + 8.))
                    .child(Scrollbar::vertical(&self.scroll_handle)),
            )
    }
}

/// A tree view element that displays hierarchical data.
#[derive(IntoElement)]
pub struct Tree<D: TreeDelegate> {
    id: ElementId,
    state: Entity<TreeState<D>>,
    style: StyleRefinement,
}

impl<D: TreeDelegate> Tree<D> {
    pub fn new(state: &Entity<TreeState<D>>) -> Self {
        Self {
            id: ElementId::Name(format!("tree-{}", state.entity_id()).into()),
            state: state.clone(),
            style: StyleRefinement::default(),
        }
    }
}

impl<D: TreeDelegate> Styled for Tree<D> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<D: TreeDelegate> RenderOnce for Tree<D> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus_handle = self.state.read(cx).focus_handle.clone();

        div()
            .id(self.id)
            .key_context(CONTEXT)
            .track_focus(&focus_handle)
            .on_action(window.listener_for(&self.state, TreeState::<D>::on_action_confirm))
            .on_action(window.listener_for(&self.state, TreeState::<D>::on_action_left))
            .on_action(window.listener_for(&self.state, TreeState::<D>::on_action_right))
            .on_action(window.listener_for(&self.state, TreeState::<D>::on_action_up))
            .on_action(window.listener_for(&self.state, TreeState::<D>::on_action_down))
            .size_full()
            .child(self.state)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};

    struct Plain;

    impl TreeDelegate for Plain {
        type Metadata = ();

        fn render_item(
            &self,
            ix: usize,
            _: &TreeEntry<()>,
            _: bool,
            _: &mut Window,
            _: &mut App,
        ) -> ListItem {
            ListItem::new(ix)
        }
    }

    fn items(open: bool) -> Vec<TreeItem<()>> {
        vec![
            TreeItem::new("a", "a", ()),
            TreeItem::new("group", "group", ())
                .expanded(open)
                .children([TreeItem::new("leaf", "leaf", ())]),
        ]
    }

    #[gpui::test]
    fn selection_follows_the_item_across_rebuilds(cx: &mut TestAppContext) {
        let tree = cx.new(|cx| TreeState::new(Plain, cx));
        tree.update(cx, |tree, cx| {
            tree.set_items(items(true), cx);
            assert_eq!(tree.select_id("leaf", cx), Some(2));

            // Collapsing the group hides the item, reopening shows it selected.
            tree.toggle_expand(1);
            assert_eq!(tree.selected_index(), None);
            tree.toggle_expand(1);
            assert_eq!(tree.selected_index(), Some(2));

            // A rebuild with a new item above keeps the same item selected.
            let mut rebuilt = vec![TreeItem::new("new", "new", ())];
            rebuilt.extend(items(true));
            tree.set_items(rebuilt, cx);
            assert_eq!(tree.selected_index(), Some(3));

            // An item not built yet is selected once it shows up.
            tree.set_items(items(false), cx);
            assert_eq!(tree.select_id("leaf", cx), None);
            tree.set_items(items(true), cx);
            assert_eq!(tree.selected_index(), Some(2));
        });
    }

    #[gpui::test]
    fn secondary_and_shift_clicks_select_several(cx: &mut TestAppContext) {
        let tree = cx.new(|cx| TreeState::new(Plain, cx));
        tree.update(cx, |tree, cx| {
            tree.set_items(items(true), cx);
            let ids = |tree: &TreeState<Plain>| {
                tree.selected_entries()
                    .iter()
                    .map(|entry| entry.item().id.to_string())
                    .collect::<Vec<_>>()
            };

            tree.select(Some(0));
            tree.toggle_selected(2);
            assert_eq!(ids(tree), ["a", "leaf"]);

            // Taking the selected item out leaves the other one selected.
            tree.toggle_selected(2);
            assert_eq!(ids(tree), ["a"]);

            tree.select_range(2);
            assert_eq!(ids(tree), ["a", "group", "leaf"]);

            // The selection is kept by id across a rebuild.
            tree.set_items(items(true), cx);
            assert_eq!(ids(tree), ["a", "group", "leaf"]);

            // A plain selection drops the others.
            tree.select(Some(1));
            assert_eq!(ids(tree), ["group"]);
        });
    }
}
