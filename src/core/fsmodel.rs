//! The node arena behind the file tree.
//!
//! Paths are stored as normalized paths relative to the root, which never
//! contain `..` (invariant I9). Node ids come from a slotmap arena and carry a
//! generation, so a stale id to a removed node is detected instead of silently
//! addressing a different node.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use slotmap::{DenseSlotMap, new_key_type};

use crate::core::baseline::ChangedEntry;
use crate::core::diff::DiffStat;

new_key_type! {
    /// Handle to a node in the arena. The key carries a generation, so a stale
    /// id to a removed node is detected instead of addressing another node.
    pub struct NodeId;
}

/// What a node is on the filesystem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    File,
    Dir,
    /// Symbolic links are shown but never followed.
    Symlink,
    /// Sockets, devices, fifos.
    Other,
}

impl NodeKind {
    pub fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }
}

/// Relation to the baseline.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Status {
    #[default]
    Unchanged,
    Modified,
    Added,
    Deleted,
}

impl Status {
    /// The word shown in the preview header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "Unchanged",
            Self::Modified => "Modified",
            Self::Added => "Added",
            Self::Deleted => "Deleted",
        }
    }

    /// The letter shown in a tree row.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Unchanged => " ",
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
        }
    }
}

/// Diff statistics of a node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeStat {
    /// Not computed yet, or no baseline is available for the node.
    Unknown,
    /// The node cannot differ from a baseline, a directory for instance.
    NotApplicable,
    Known(DiffStat),
}

/// A single filesystem node.
#[derive(Clone, Debug)]
pub struct Node {
    /// Base name, never sanitized at this point (invariant I8).
    pub name: OsString,
    pub parent: Option<NodeId>,
    /// Normalized path relative to the root.
    pub rel: PathBuf,
    pub kind: NodeKind,
    pub status: Status,
    pub stat: NodeStat,
    /// Last time a change was observed, or the mtime at startup.
    pub changed_at: Option<SystemTime>,
    /// True once [`FsModel::merge_children`] has seen the directory listing.
    pub loaded: bool,
    /// A placeholder parent created to reach a changed descendant.
    pub skeleton: bool,
    /// True when the node could not be read, shown with a `!` marker.
    pub unreadable: bool,
    /// Target of a symbolic link, for display only.
    pub link_target: Option<OsString>,
    children: Vec<NodeId>,
}

impl Node {
    fn new(name: OsString, parent: Option<NodeId>, rel: PathBuf, kind: NodeKind) -> Self {
        Self {
            name,
            parent,
            rel,
            kind,
            status: Status::Unchanged,
            stat: NodeStat::NotApplicable,
            changed_at: None,
            loaded: false,
            skeleton: false,
            unreadable: false,
            link_target: None,
            children: Vec::new(),
        }
    }

    /// Directories can be expanded; symlinks never are.
    pub fn is_expandable(&self) -> bool {
        self.kind.is_dir()
    }
}

/// One entry as reported by the filesystem.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DirEntry {
    pub name: OsString,
    pub kind: NodeKind,
    pub link_target: Option<OsString>,
    pub unreadable: bool,
}

/// The arena of all known nodes.
pub struct FsModel {
    arena: DenseSlotMap<NodeId, Node>,
    index: HashMap<PathBuf, NodeId>,
    root_id: NodeId,
    root_path: PathBuf,
}

impl FsModel {
    /// Creates a model whose only node is the root directory.
    pub fn new(root: PathBuf) -> Self {
        let mut arena = DenseSlotMap::with_key();
        let root_node = arena.insert(Node::new(
            root.file_name()
                .unwrap_or_else(|| OsStr::new("/"))
                .to_os_string(),
            None,
            PathBuf::new(),
            NodeKind::Dir,
        ));
        let mut index = HashMap::new();
        index.insert(PathBuf::new(), root_node);
        Self {
            arena,
            index,
            root_id: root_node,
            root_path: root,
        }
    }

    pub fn root_id(&self) -> NodeId {
        self.root_id
    }

    /// Absolute path of the watched root.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.arena.get(id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.arena.get_mut(id)
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.arena.contains_key(id)
    }

    /// Number of nodes in the arena.
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }

    /// All live nodes, in arena order.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.arena.iter()
    }

    /// Path relative to the root.
    pub fn rel_of(&self, id: NodeId) -> Option<&Path> {
        self.arena.get(id).map(|node| node.rel.as_path())
    }

    /// Path of `id` on the filesystem.
    pub fn absolute(&self, id: NodeId) -> Option<PathBuf> {
        self.arena
            .get(id)
            .map(|node| self.root_path.join(&node.rel))
    }

    /// Finds a node by its path relative to the root.
    pub fn lookup(&self, rel: &Path) -> Option<NodeId> {
        self.index.get(rel).copied()
    }

    /// Reorders the children of `id`. The ids must already be children.
    pub fn set_children(&mut self, id: NodeId, children: Vec<NodeId>) {
        if let Some(node) = self.arena.get_mut(id) {
            node.children = children;
        }
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.arena
            .get(id)
            .map(|node| node.children.as_slice())
            .unwrap_or(&[])
    }

    /// Inserts a child, reusing the node that already owns that name so that
    /// expansion state and selection survive a reload.
    pub fn child_id(&self, parent: NodeId, name: &OsStr) -> Option<NodeId> {
        let rel = self.rel_of(parent)?.join(name);
        self.index.get(&rel).copied()
    }

    /// Creates a child node.
    pub fn insert_child(&mut self, parent: NodeId, name: &OsStr, kind: NodeKind) -> NodeId {
        let rel = self
            .rel_of(parent)
            .map_or_else(|| PathBuf::from(name), |parent_rel| parent_rel.join(name));
        if let Some(existing) = self.index.get(&rel).copied() {
            return existing;
        }
        let id = self.arena.insert(Node::new(
            name.to_os_string(),
            Some(parent),
            rel.clone(),
            kind,
        ));
        self.index.insert(rel, id);
        if let Some(node) = self.arena.get_mut(parent) {
            node.children.push(id);
        }
        id
    }

    /// Replaces the children of `parent` with `entries`.
    ///
    /// Nodes whose name is still present keep their identity; nodes that
    /// disappeared are removed together with their subtree. Returns the ids of
    /// the children in listing order; the caller is responsible for sorting.
    pub fn merge_children(&mut self, parent: NodeId, entries: &[DirEntry]) -> Vec<NodeId> {
        let mut result = Vec::with_capacity(entries.len());
        let mut keep: Vec<NodeId> = Vec::with_capacity(entries.len());

        for entry in entries {
            let id = self
                .child_id(parent, &entry.name)
                .unwrap_or_else(|| self.insert_child(parent, &entry.name, entry.kind));
            if let Some(node) = self.arena.get_mut(id) {
                node.kind = entry.kind;
                node.link_target = entry.link_target.clone();
                node.unreadable = entry.unreadable;
                // A node that really exists is never a filter placeholder.
                node.skeleton = false;
                node.loaded = entry.kind.is_dir() && node.loaded;
            }
            keep.push(id);
            result.push(id);
        }

        let stale: Vec<NodeId> = {
            let keep: std::collections::HashSet<NodeId> = keep.iter().copied().collect();
            self.children(parent)
                .iter()
                .copied()
                .filter(|id| !keep.contains(id))
                .collect()
        };
        for id in stale {
            self.remove(id);
        }
        if let Some(node) = self.arena.get_mut(parent) {
            node.children = result.clone();
            node.loaded = true;
            node.skeleton = false;
        }
        result
    }

    /// Removes a node and its whole subtree.
    pub fn remove(&mut self, id: NodeId) {
        let children = self.children(id).to_vec();
        for child in children {
            self.remove(child);
        }
        if let Some(node) = self.arena.remove(id) {
            self.index.remove(&node.rel);
            if let Some(parent) = node.parent
                && let Some(node) = self.arena.get_mut(parent)
            {
                node.children.retain(|child| *child != id);
            }
        }
    }

    /// Marks `id` as a placeholder that stands in for a loaded descendant.
    pub fn mark_skeleton(&mut self, id: NodeId) {
        if let Some(node) = self.arena.get_mut(id) {
            node.skeleton = true;
        }
    }

    /// Applies the baseline's changed set to the tree (design §11.1).
    ///
    /// Every node takes its status back from the set, and a file that exists
    /// only in the baseline keeps a node of its own so the tree can show it
    /// as `D`. A changed file inside a directory no listing has reached yet
    /// simply waits for that listing; the next call covers it.
    pub fn sync_baseline(&mut self, changed: &BTreeMap<PathBuf, ChangedEntry>) {
        for (_, node) in self.arena.iter_mut() {
            node.status = Status::Unchanged;
        }
        for (rel, entry) in changed {
            if let Some(id) = self.lookup(rel) {
                if let Some(node) = self.arena.get_mut(id) {
                    node.status = entry.status;
                    node.changed_at = Some(entry.changed_at);
                }
            } else if entry.status == Status::Deleted {
                self.synthesize_deleted(rel, entry);
            }
        }
    }

    /// Adds the node of a file that only exists in the baseline, together
    /// with any directory above it that no listing has created yet.
    fn synthesize_deleted(&mut self, rel: &Path, entry: &ChangedEntry) {
        let mut names = Vec::new();
        for component in rel.components() {
            let std::path::Component::Normal(name) = component else {
                // Paths are relative and normalized (invariant I9): anything
                // else has no node to stand under.
                return;
            };
            names.push(name);
        }
        let Some((last, rest)) = names.split_last() else {
            return;
        };
        let mut parent = self.root_id();
        for name in rest {
            match self.child_id(parent, name) {
                Some(id) => parent = id,
                None => {
                    let id = self.insert_child(parent, name, NodeKind::Dir);
                    // The directory never appeared in a listing, so it is a
                    // placeholder standing in for the changed file below it.
                    self.mark_skeleton(id);
                    parent = id;
                }
            }
        }
        let id = match self.child_id(parent, last) {
            Some(id) => id,
            None => self.insert_child(parent, last, NodeKind::File),
        };
        if let Some(node) = self.arena.get_mut(id) {
            node.status = entry.status;
            node.changed_at = Some(entry.changed_at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    fn model() -> FsModel {
        FsModel::new(PathBuf::from("/root"))
    }

    #[test]
    fn root_is_a_directory_node() {
        let model = model();
        let root = model.get(model.root_id()).expect("root exists");
        assert_eq!(root.kind, NodeKind::Dir);
        assert_eq!(root.rel, PathBuf::new());
        assert_eq!(
            model.absolute(model.root_id()).as_deref(),
            Some(Path::new("/root"))
        );
    }

    #[test]
    fn relative_paths_are_built_without_dot_dot() {
        let mut model = model();
        let a = model.insert_child(model.root_id(), os("a"), NodeKind::Dir);
        let b = model.insert_child(a, os("b.rs"), NodeKind::File);
        assert_eq!(model.rel_of(b), Some(Path::new("a/b.rs")));
        assert_eq!(model.lookup(Path::new("a/b.rs")), Some(b));
    }

    #[test]
    fn inserting_the_same_name_twice_reuses_the_node() {
        let mut model = model();
        let first = model.insert_child(model.root_id(), os("a"), NodeKind::File);
        let second = model.insert_child(model.root_id(), os("a"), NodeKind::File);
        assert_eq!(first, second);
        assert_eq!(model.children(model.root_id()).len(), 1);
    }

    #[test]
    fn merge_keeps_surviving_nodes_and_drops_the_rest() {
        let mut model = model();
        let keep = model.insert_child(model.root_id(), os("keep"), NodeKind::File);
        let drop = model.insert_child(model.root_id(), os("drop"), NodeKind::File);
        let entries = vec![
            DirEntry {
                name: os("keep").to_os_string(),
                kind: NodeKind::Dir,
                link_target: None,
                unreadable: false,
            },
            DirEntry {
                name: os("new").to_os_string(),
                kind: NodeKind::File,
                link_target: None,
                unreadable: false,
            },
        ];
        let children = model.merge_children(model.root_id(), &entries);
        assert_eq!(children.len(), 2);
        assert_eq!(children[0], keep);
        assert!(!model.contains(drop));
        assert_eq!(model.get(keep).map(|node| node.kind), Some(NodeKind::Dir));
        assert!(model.get(model.root_id()).is_some_and(|node| node.loaded));
    }

    #[test]
    fn remove_drops_the_whole_subtree() {
        let mut model = model();
        let a = model.insert_child(model.root_id(), os("a"), NodeKind::Dir);
        let b = model.insert_child(a, os("b"), NodeKind::File);
        model.remove(a);
        assert!(!model.contains(a));
        assert!(!model.contains(b));
        assert_eq!(model.lookup(Path::new("a/b")), None);
        assert!(model.children(model.root_id()).is_empty());
    }

    #[test]
    fn stale_ids_are_rejected_after_removal() {
        let mut model = model();
        let a = model.insert_child(model.root_id(), os("a"), NodeKind::File);
        model.remove(a);
        let b = model.insert_child(model.root_id(), os("a"), NodeKind::File);
        assert_ne!(a, b);
        assert!(model.get(a).is_none());
        assert!(model.get(b).is_some());
    }

    fn changed(status: Status) -> ChangedEntry {
        ChangedEntry {
            status,
            changed_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_changed_set_marks_the_nodes_it_names_and_forgets_them_again() {
        let mut model = model();
        let a = model.insert_child(model.root_id(), os("a.txt"), NodeKind::File);
        let keep = model.insert_child(model.root_id(), os("keep.txt"), NodeKind::File);
        model.sync_baseline(&BTreeMap::from([(
            PathBuf::from("a.txt"),
            changed(Status::Modified),
        )]));
        assert_eq!(
            model.get(a).map(|node| (node.status, node.changed_at)),
            Some((Status::Modified, Some(SystemTime::UNIX_EPOCH)))
        );
        assert_eq!(
            model.get(keep).map(|node| node.status),
            Some(Status::Unchanged)
        );
        model.sync_baseline(&BTreeMap::new());
        assert_eq!(
            model.get(a).map(|node| node.status),
            Some(Status::Unchanged),
            "a rebuild that no longer lists the file keeps its old status"
        );
    }

    #[test]
    fn a_deleted_file_keeps_a_node_under_a_directory_no_listing_has_reached() {
        let mut model = model();
        model.sync_baseline(&BTreeMap::from([
            (PathBuf::from("gone.txt"), changed(Status::Deleted)),
            (PathBuf::from("deep/gone.txt"), changed(Status::Deleted)),
        ]));
        let gone = model
            .lookup(Path::new("gone.txt"))
            .expect("the deleted file keeps a row");
        assert_eq!(
            model.get(gone).map(|node| (node.kind, node.status)),
            Some((NodeKind::File, Status::Deleted))
        );
        let nested = model
            .lookup(Path::new("deep/gone.txt"))
            .expect("the row below the unlisted directory");
        assert_eq!(
            model.get(nested).map(|node| (node.kind, node.status)),
            Some((NodeKind::File, Status::Deleted))
        );
        let deep = model
            .lookup(Path::new("deep"))
            .expect("the directory above it");
        assert_eq!(
            model.get(deep).map(|node| (node.kind, node.skeleton)),
            Some((NodeKind::Dir, true)),
            "an unlisted directory is a placeholder for the rows below it"
        );
    }

    #[test]
    fn a_changed_file_without_a_node_waits_for_its_listing() {
        let mut model = model();
        model.sync_baseline(&BTreeMap::from([(
            PathBuf::from("not_yet.txt"),
            changed(Status::Modified),
        )]));
        // Only a deletion can stand on the baseline alone: every other
        // status needs a node, and the listing creates it.
        assert!(model.lookup(Path::new("not_yet.txt")).is_none());
        let node = model.insert_child(model.root_id(), os("not_yet.txt"), NodeKind::File);
        model.sync_baseline(&BTreeMap::from([(
            PathBuf::from("not_yet.txt"),
            changed(Status::Modified),
        )]));
        assert_eq!(
            model.get(node).map(|node| node.status),
            Some(Status::Modified)
        );
    }
}
