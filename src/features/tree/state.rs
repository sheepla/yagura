//! Tree state: expansion, selection, scrolling and the visible row cache.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::app::state::SortMode;
use crate::core::fsmodel::{FsModel, NodeId, Status};

/// Hard cap on the flattened tree so that a pathological tree cannot exhaust
/// memory. Rows past the cap are not drawn.
pub const MAX_VISIBLE_ROWS: usize = 200_000;

/// One line of the flattened tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VisibleRow {
    pub node: NodeId,
    /// Indentation depth, the root being 0.
    pub depth: u16,
}

pub struct TreeState {
    pub model: FsModel,
    /// Directories whose children are shown.
    pub expanded: HashSet<NodeId>,
    /// Selection by identity, never by index (invariant I4).
    pub selected: Option<NodeId>,
    /// Index of the first visible row.
    pub scroll: usize,
    /// Rows of the tree, rebuilt when `ver` changes.
    pub visible: Vec<VisibleRow>,
    /// Structural version of the tree.
    pub ver: u64,
    /// Version `visible` was built from.
    pub cached_ver: u64,
    /// Per node generation counter for directory loads (invariant I3).
    pub load_gen: HashMap<NodeId, u64>,
    /// Nodes with a directory load in flight.
    pub loading: HashSet<NodeId>,
    /// Nodes whose listing was asked for again while one was already in
    /// flight: the answer in hand predates the change that asked, so once it
    /// lands the load is issued again rather than left to the next event.
    pub pending_relist: HashSet<NodeId>,
    /// Sibling order.
    pub sort: SortMode,
    /// Only the rows that changed or were seen to change (`c`, design §11.3).
    pub changed_only: bool,
    /// Rows that fit on screen, kept in step with the layout by the reducer.
    pub viewport: usize,
    /// A path a finder has asked to be shown, until the listings on the way
    /// to it have arrived and the row exists to select (§4).
    pub jump: Option<PathBuf>,
}

impl TreeState {
    pub fn new(model: FsModel) -> Self {
        let root = model.root_id();
        Self {
            model,
            expanded: HashSet::new(),
            selected: Some(root),
            scroll: 0,
            visible: Vec::new(),
            ver: 0,
            cached_ver: u64::MAX,
            load_gen: HashMap::new(),
            loading: HashSet::new(),
            pending_relist: HashSet::new(),
            sort: SortMode::Name,
            changed_only: false,
            viewport: 0,
            jump: None,
        }
    }

    /// Marks the tree as changed so that the visible rows are rebuilt.
    pub fn bump(&mut self) {
        self.ver = self.ver.wrapping_add(1);
    }

    /// Rows of the flattened tree, rebuilt only after a change.
    ///
    /// The scroll offset is corrected so that the selected node keeps its
    /// position on screen, which is what stops the cursor from jumping when the
    /// filesystem changes underneath (design 11.4).
    pub fn refresh(&mut self) {
        if self.cached_ver == self.ver {
            return;
        }
        let screen_row = self
            .selected_index()
            .map(|index| index as isize - self.scroll as isize);
        self.rebuild_visible();
        if let Some(row) = screen_row
            && let Some(index) = self.selected_index()
        {
            let target = index as isize - row;
            self.scroll = target.max(0) as usize;
        }
        self.clamp_scroll();
        self.cached_ver = self.ver;
    }

    fn rebuild_visible(&mut self) {
        let mut rows = Vec::new();
        self.walk(self.model.root_id(), 0, &mut rows);
        self.visible = rows;
    }

    /// Adds a row for `node` and, when it is open, for everything below it.
    ///
    /// The row is kept only when the filter has something to say about the
    /// subtree it heads, so with `changed_only` on a directory survives for
    /// the change below it and the path to that change stays walkable
    /// (design §11.3). Returns whether the subtree matched.
    fn walk(&self, node: NodeId, depth: u16, out: &mut Vec<VisibleRow>) -> bool {
        if out.len() >= MAX_VISIBLE_ROWS {
            return false;
        }
        let start = out.len();
        out.push(VisibleRow { node, depth });
        // The root frames everything else and stays even when the filter
        // finds nothing to show under it.
        let mut matched = node == self.model.root_id() || self.keeps(node);
        if self.is_expanded(node) {
            for child in self.model.children(node) {
                matched |= self.walk(*child, depth + 1, out);
            }
        }
        if !matched {
            out.truncate(start);
        }
        matched
    }

    /// Whether a row is shown while `changed_only` is on.
    fn keeps(&self, node: NodeId) -> bool {
        if !self.changed_only {
            return true;
        }
        self.model
            .get(node)
            .is_some_and(|entry| entry.status != Status::Unchanged || entry.changed_at.is_some())
    }

    pub fn is_expanded(&self, node: NodeId) -> bool {
        self.expanded.contains(&node)
    }

    pub fn expand(&mut self, node: NodeId) {
        if self
            .model
            .get(node)
            .is_some_and(|node| node.is_expandable())
            && self.expanded.insert(node)
        {
            self.bump();
        }
    }

    pub fn collapse(&mut self, node: NodeId) {
        if self.expanded.remove(&node) {
            self.bump();
        }
    }

    pub fn toggle(&mut self, node: NodeId) {
        if self.is_expanded(node) {
            self.collapse(node);
        } else {
            self.expand(node);
        }
    }

    /// Forgets the expansion of every descendant of `node`.
    pub fn collapse_descendants(&mut self, node: NodeId) {
        let mut stack: Vec<NodeId> = self.model.children(node).to_vec();
        let mut removed = false;
        while let Some(id) = stack.pop() {
            stack.extend_from_slice(self.model.children(id));
            removed |= self.expanded.remove(&id);
        }
        if removed {
            self.bump();
        }
    }

    /// Index of the selection in [`Self::visible`].
    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.visible.iter().position(|row| row.node == selected)
    }

    /// Selects `node` if it is different from the current selection.
    pub fn select(&mut self, node: NodeId) {
        if self.selected != Some(node) {
            self.selected = Some(node);
            self.bump();
        }
    }

    /// Moves the selection by `delta` rows, stopping at both ends.
    pub fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let current = self.selected_index().unwrap_or(0) as isize;
        let last = self.visible.len() as isize - 1;
        let target = (current + delta).clamp(0, last);
        self.select(self.visible[target as usize].node);
        self.scroll_into_view(target as usize);
    }

    pub fn select_first(&mut self) {
        if let Some(row) = self.visible.first() {
            self.select(row.node);
            self.scroll = 0;
        }
    }

    pub fn select_last(&mut self) {
        if let Some(row) = self.visible.last() {
            self.select(row.node);
            self.scroll_into_view(self.visible.len() - 1);
        }
    }

    /// Moves the selection by a viewport, forwards or backwards.
    pub fn page(&mut self, forward: bool) {
        let step = self.viewport.max(1) as isize;
        self.move_selection(if forward { step } else { -step });
    }

    /// Moves the selection by half a viewport, forwards or backwards.
    pub fn half_page(&mut self, forward: bool) {
        let step = (self.viewport.max(2) / 2).max(1) as isize;
        self.move_selection(if forward { step } else { -step });
    }

    /// Scrolls the least amount that keeps row `index` on screen.
    pub fn scroll_into_view(&mut self, index: usize) {
        let height = self.viewport.max(1);
        if index < self.scroll {
            self.scroll = index;
        } else if index >= self.scroll + height {
            self.scroll = index + 1 - height;
        }
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let max = self.max_scroll();
        if self.scroll > max {
            self.scroll = max;
        }
    }

    pub fn max_scroll(&self) -> usize {
        self.visible.len().saturating_sub(self.viewport.max(1))
    }

    /// Records how many rows fit on screen. Zero means the tree is hidden.
    pub fn set_viewport(&mut self, height: u16) {
        self.viewport = height as usize;
        if height == 0 {
            self.scroll = 0;
        }
        self.clamp_scroll();
    }

    /// Number of rows the tree currently has, the status bar denominator.
    pub fn row_count(&self) -> usize {
        self.visible.len()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use crate::core::fsmodel::{DirEntry, NodeKind};

    use super::super::sort::sort_children;
    use super::*;

    fn entries(names: &[(&str, NodeKind)]) -> Vec<DirEntry> {
        names
            .iter()
            .map(|(name, kind)| DirEntry {
                name: OsStr::new(name).to_os_string(),
                kind: *kind,
                link_target: None,
                unreadable: false,
            })
            .collect()
    }

    /// Builds this tree, with the root expanded:
    ///
    /// ```text
    /// root
    ///   src
    ///     main.rs
    ///   readme.md
    /// ```
    fn sample() -> TreeState {
        let mut model = FsModel::new(PathBuf::from("/root"));
        let root = model.root_id();
        let top = entries(&[("src", NodeKind::Dir), ("readme.md", NodeKind::File)]);
        let children = model.merge_children(root, &top);
        sort_children(&mut model, root, SortMode::Name);
        let src = children[0];
        model.merge_children(src, &entries(&[("main.rs", NodeKind::File)]));
        let mut tree = TreeState::new(model);
        tree.expand(root);
        tree.refresh();
        tree
    }

    fn names(tree: &TreeState) -> Vec<String> {
        tree.visible
            .iter()
            .map(|row| {
                let node = tree.model.get(row.node).expect("node");
                format!(
                    "{}{}",
                    "  ".repeat(row.depth as usize),
                    node.name.to_string_lossy()
                )
            })
            .collect()
    }

    fn src(tree: &TreeState) -> NodeId {
        tree.model.lookup(Path::new("src")).expect("src node")
    }

    #[test]
    fn a_new_tree_shows_only_the_root() {
        let mut tree = TreeState::new(FsModel::new(PathBuf::from("/root")));
        tree.refresh();
        assert_eq!(tree.row_count(), 1);
        assert_eq!(tree.selected, Some(tree.model.root_id()));
    }

    #[test]
    fn directories_sort_ahead_of_files_and_carry_their_depth() {
        let tree = sample();
        assert_eq!(names(&tree), vec!["root", "  src", "  readme.md"]);
    }

    #[test]
    fn expansion_reveals_children_and_collapsing_hides_them() {
        let mut tree = sample();
        assert_eq!(tree.row_count(), 3);
        tree.expand(src(&tree));
        tree.refresh();
        assert_eq!(
            names(&tree),
            vec!["root", "  src", "    main.rs", "  readme.md"]
        );
        tree.collapse(src(&tree));
        tree.refresh();
        assert_eq!(tree.row_count(), 3);
    }

    #[test]
    fn movement_stops_at_the_ends() {
        let mut tree = sample();
        tree.set_viewport(2);
        tree.move_selection(-5);
        assert_eq!(tree.selected_index(), Some(0));
        tree.move_selection(99);
        assert_eq!(tree.selected_index(), Some(tree.row_count() - 1));
        tree.move_selection(-1);
        assert_eq!(tree.selected_index(), Some(tree.row_count() - 2));
    }

    #[test]
    fn the_selected_row_keeps_its_line_when_a_row_appears_above_it() {
        let mut tree = sample();
        tree.set_viewport(3);
        tree.move_selection(1);
        tree.move_selection(1);
        let screen_row = tree.selected_index().expect("index") - tree.scroll;
        assert_eq!(tree.scroll, 0);
        assert_eq!(screen_row, 2);

        let root = tree.model.root_id();
        // A new file shows up between `src` and the selected `readme.md`.
        tree.model.merge_children(
            root,
            &entries(&[
                ("src", NodeKind::Dir),
                ("aaa.rs", NodeKind::File),
                ("readme.md", NodeKind::File),
            ]),
        );
        sort_children(&mut tree.model, root, tree.sort);
        tree.bump();
        tree.refresh();

        let selected = tree
            .model
            .get(tree.selected.expect("selection"))
            .expect("node");
        assert_eq!(selected.name.to_string_lossy(), "readme.md");
        assert_eq!(tree.row_count(), 4);
        assert_eq!(
            tree.selected_index().expect("index") - tree.scroll,
            screen_row
        );
    }

    #[test]
    fn paging_moves_by_the_viewport_and_half_a_viewport() {
        let mut tree = sample();
        tree.set_viewport(2);
        tree.page(true);
        assert_eq!(tree.selected_index(), Some(2));
        tree.page(false);
        assert_eq!(tree.selected_index(), Some(0));
        tree.half_page(true);
        assert_eq!(tree.selected_index(), Some(1));
    }

    #[test]
    fn scrolling_follows_the_selection_and_is_clamped() {
        let mut tree = sample();
        tree.set_viewport(2);
        tree.select_last();
        assert_eq!(tree.scroll, tree.max_scroll());
        assert!(tree.scroll + tree.viewport >= tree.row_count());
        tree.select_first();
        assert_eq!(tree.scroll, 0);
    }

    #[test]
    fn hiding_the_tree_resets_the_scroll_offset() {
        let mut tree = sample();
        tree.set_viewport(1);
        tree.select_last();
        assert!(tree.scroll > 0);
        tree.set_viewport(0);
        assert_eq!(tree.scroll, 0);
    }

    #[test]
    fn collapse_descendants_forgets_nested_expansions() {
        let mut tree = sample();
        let src = src(&tree);
        tree.expand(src);
        assert!(tree.is_expanded(src));
        tree.collapse_descendants(tree.model.root_id());
        assert!(!tree.is_expanded(src));
    }

    #[test]
    fn symlinks_are_never_expanded() {
        let mut model = FsModel::new(PathBuf::from("/root"));
        let root = model.root_id();
        model.merge_children(root, &entries(&[("link", NodeKind::Symlink)]));
        let mut tree = TreeState::new(model);
        tree.expand(root);
        let link = tree.model.children(root)[0];
        tree.expand(link);
        tree.bump();
        tree.refresh();
        assert!(!tree.is_expanded(link));
        assert_eq!(tree.row_count(), 2);
    }

    #[test]
    fn the_filter_shows_what_changed_and_the_way_to_it() {
        let mut tree = sample();
        let src = src(&tree);
        tree.expand(src);
        // A directory carries the change below it, the way the baseline's set
        // writes it (design §11.1), so it survives the filter even closed.
        let now = std::time::SystemTime::UNIX_EPOCH;
        tree.model.get_mut(src).expect("src").changed_at = Some(now);
        let main = tree.model.lookup(Path::new("src/main.rs")).expect("main");
        tree.model.get_mut(main).expect("main").status = Status::Modified;

        tree.changed_only = true;
        tree.bump();
        tree.refresh();
        assert_eq!(names(&tree), vec!["root", "  src", "    main.rs"]);
        assert_eq!(tree.row_count(), 3, "readme.md did not change");

        tree.collapse(src);
        tree.refresh();
        assert_eq!(names(&tree), vec!["root", "  src"], "a closed directory");
    }

    #[test]
    fn a_row_seen_to_change_is_shown_by_the_filter_too() {
        let mut tree = sample();
        let readme = tree.model.lookup(Path::new("readme.md")).expect("readme");
        tree.model.get_mut(readme).expect("readme").changed_at =
            Some(std::time::SystemTime::UNIX_EPOCH);

        tree.changed_only = true;
        tree.bump();
        tree.refresh();
        assert_eq!(names(&tree), vec!["root", "  readme.md"]);
    }

    #[test]
    fn the_filter_keeps_the_root_and_comes_off_again() {
        let mut tree = sample();
        tree.changed_only = true;
        tree.bump();
        tree.refresh();
        assert_eq!(names(&tree), vec!["root"]);

        tree.changed_only = false;
        tree.bump();
        tree.refresh();
        assert_eq!(names(&tree), vec!["root", "  src", "  readme.md"]);
    }
}
