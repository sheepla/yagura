//! Sibling order inside the tree.
//!
//! Directories come first, then names in natural order: case insensitive, with
//! digit runs compared as numbers so that `file2` sorts before `file10`.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::time::SystemTime;

use crate::app::state::SortMode;
use crate::core::fsmodel::{FsModel, NodeId, NodeKind};

/// Sorts the children of `node` in place.
pub fn sort_children(model: &mut FsModel, node: NodeId, mode: SortMode) {
    if !model.contains(node) {
        return;
    }
    let mut rows: Vec<(NodeId, Sibling)> = model
        .children(node)
        .iter()
        .filter_map(|id| {
            model.get(*id).map(|child| {
                (
                    *id,
                    Sibling {
                        kind: child.kind,
                        name: child.name.clone(),
                        changed_at: child.changed_at,
                    },
                )
            })
        })
        .collect();
    rows.sort_by(|(_, a), (_, b)| match mode {
        SortMode::Name => a.cmp_against(b),
        // Directories first as usual; files follow by change time, newest first,
        // and the name only breaks a tie so the order stays deterministic.
        SortMode::Recent => {
            let kind = b.kind.is_dir().cmp(&a.kind.is_dir());
            kind.then_with(|| {
                let by_time = b.changed_at.cmp(&a.changed_at);
                if by_time == Ordering::Equal {
                    compare_names(&a.name, &b.name)
                } else {
                    by_time
                }
            })
        }
    });
    model.set_children(node, rows.into_iter().map(|(id, _)| id).collect());
}

/// Applies the order to every node of the tree.
pub fn sort_all(model: &mut FsModel, mode: SortMode) {
    let ids: Vec<NodeId> = model.iter().map(|(id, _)| id).collect();
    for id in ids {
        sort_children(model, id, mode);
    }
}

/// Natural, case insensitive name comparison.
pub fn compare_names(a: &OsStr, b: &OsStr) -> Ordering {
    compare_natural(&a.to_string_lossy(), &b.to_string_lossy())
}

fn compare_natural(a: &str, b: &str) -> Ordering {
    let left = a.to_lowercase();
    let right = b.to_lowercase();
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => {
                if l.is_ascii_digit() && r.is_ascii_digit() {
                    let l_digits = take_digits(&mut left);
                    let r_digits = take_digits(&mut right);
                    match compare_numbers(&l_digits, &r_digits) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
                left.next();
                right.next();
                if l != r {
                    return l.cmp(&r);
                }
            }
        }
    }
}

/// One child of a node, reduced to the fields the order depends on.
struct Sibling {
    kind: NodeKind,
    name: std::ffi::OsString,
    changed_at: Option<SystemTime>,
}

impl Sibling {
    fn cmp_against(&self, other: &Self) -> Ordering {
        other
            .kind
            .is_dir()
            .cmp(&self.kind.is_dir())
            .then_with(|| compare_names(&self.name, &other.name))
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(c) = chars.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        digits.push(*c);
        chars.next();
    }
    digits
}

fn compare_numbers(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn model_with(names: &[(&str, NodeKind)]) -> (FsModel, NodeId) {
        let mut model = FsModel::new(PathBuf::from("/root"));
        let root = model.root_id();
        for (name, kind) in names {
            model.insert_child(root, OsStr::new(name), *kind);
        }
        (model, root)
    }

    fn order(model: &FsModel) -> Vec<String> {
        model
            .children(model.root_id())
            .iter()
            .map(|id| {
                model
                    .get(*id)
                    .expect("node")
                    .name
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn directories_come_first() {
        let (mut model, root) = model_with(&[
            ("b.rs", NodeKind::File),
            ("a", NodeKind::Dir),
            ("a.rs", NodeKind::File),
        ]);
        sort_children(&mut model, root, SortMode::Name);
        assert_eq!(order(&model), vec!["a", "a.rs", "b.rs"]);
    }

    #[test]
    fn names_sort_naturally_and_case_insensitively() {
        let (mut model, root) = model_with(&[
            ("File10.rs", NodeKind::File),
            ("file2.rs", NodeKind::File),
            ("File1.rs", NodeKind::File),
            ("apple.rs", NodeKind::File),
        ]);
        sort_children(&mut model, root, SortMode::Name);
        assert_eq!(
            order(&model),
            vec!["apple.rs", "File1.rs", "file2.rs", "File10.rs"]
        );
    }

    #[test]
    fn numbers_are_compared_by_value_not_by_text() {
        assert_eq!(
            compare_names(OsStr::new("x9"), OsStr::new("x10")),
            Ordering::Less
        );
        assert_eq!(
            compare_names(OsStr::new("x007"), OsStr::new("x7")),
            Ordering::Equal
        );
        assert_eq!(
            compare_names(OsStr::new("x12b"), OsStr::new("x12a")),
            Ordering::Greater
        );
        assert_eq!(
            compare_names(OsStr::new(""), OsStr::new("a")),
            Ordering::Less
        );
        assert_eq!(
            compare_names(OsStr::new("abc"), OsStr::new("abc")),
            Ordering::Equal
        );
    }

    #[test]
    fn recent_mode_orders_siblings_by_change_time() {
        let (mut model, root) = model_with(&[("a", NodeKind::File), ("b", NodeKind::File)]);
        let a = model.child_id(root, OsStr::new("a")).expect("a");
        let b = model.child_id(root, OsStr::new("b")).expect("b");
        model.get_mut(a).expect("a").changed_at = Some(SystemTime::UNIX_EPOCH);
        model.get_mut(b).expect("b").changed_at =
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60));
        sort_children(&mut model, root, SortMode::Recent);
        assert_eq!(order(&model), vec!["b", "a"]);
        sort_children(&mut model, root, SortMode::Name);
        assert_eq!(order(&model), vec!["a", "b"]);
    }

    #[test]
    fn sorting_a_removed_node_is_harmless() {
        let (mut model, root) = model_with(&[("a", NodeKind::File)]);
        sort_all(&mut model, SortMode::Recent);
        model.remove(root);
        sort_children(&mut model, root, SortMode::Name);
    }

    #[test]
    fn non_utf8_names_are_ordered_without_panicking() {
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let a = OsStr::from_bytes(b"a\xff");
            let b = OsStr::from_bytes(b"a\xfe");
            let _ = compare_names(a, b);
        }
    }
}
