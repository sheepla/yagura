//! Property tests for the tree.
//!
//! These do not check what a command is supposed to do, they check the
//! invariants the rest of the program leans on: the row cache is never stale,
//! the selection is always on screen, a load is only ever asked for a
//! directory, and a listing that is not the newest one changes nothing.

mod support;

use std::collections::HashSet;

use proptest::prelude::*;
use yagura::app::state::AppState;
use yagura::app::{Action, Command, Effect, reduce::reduce};
use yagura::core::dirload::DirError;
use yagura::core::fsmodel::NodeId;
use yagura::core::preview::{Doc, Language};
use yagura::features::preview::PreviewStatus;

use support::{fixture_entries, plain_state};

/// A tree with three entries under the root, as if the root listing had come
/// back from the filesystem.
fn populated() -> AppState {
    let mut state = plain_state(std::path::Path::new("root"), false);
    let root = state.tree.model.root_id();
    let generation = state.tree.load_gen[&root];
    reduce(
        &mut state,
        Action::DirLoaded {
            node: root,
            generation,
            result: Ok(fixture_entries()),
        },
    );
    state
}

/// The commands that move the selection or change the shape of the tree.
fn tree_command() -> impl Strategy<Value = Command> {
    prop_oneof![
        Just(Command::MoveDown),
        Just(Command::MoveUp),
        Just(Command::PageDown),
        Just(Command::PageUp),
        Just(Command::HalfPageDown),
        Just(Command::HalfPageUp),
        Just(Command::First),
        Just(Command::Last),
        Just(Command::Expand),
        Just(Command::Collapse),
        Just(Command::ToggleExpand),
        Just(Command::ToggleTree),
    ]
}

/// The rows the tree should have, counted from the model and the set of
/// expanded directories rather than from the cache itself.
fn reachable_rows(state: &AppState) -> HashSet<NodeId> {
    let mut rows = HashSet::new();
    let mut stack = vec![state.tree.model.root_id()];
    while let Some(node) = stack.pop() {
        rows.insert(node);
        if state.tree.is_expanded(node) {
            stack.extend_from_slice(state.tree.model.children(node));
        }
    }
    rows
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        // Named so that a failure is replayed even though the tests live
        // outside src/.
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "tests/tree_properties.proptest-regressions",
            ),
        )),
        ..ProptestConfig::default()
    })]

    /// The cache of visible rows always matches the tree it was built from.
    #[test]
    fn the_visible_rows_are_always_in_step_with_the_tree(
        commands in prop::collection::vec(tree_command(), 1..24)
    ) {
        let mut state = populated();
        for command in commands {
            reduce(&mut state, Action::Command(command));
        }
        state.tree.refresh();
        prop_assert_eq!(state.tree.cached_ver, state.tree.ver);
        let rows: HashSet<NodeId> = state.tree.visible.iter().map(|row| row.node).collect();
        prop_assert_eq!(rows, reachable_rows(&state));
    }

    /// Every row is a node of the model, at a depth that matches the tree.
    #[test]
    fn every_row_is_a_real_node_at_the_right_depth(
        commands in prop::collection::vec(tree_command(), 1..24)
    ) {
        let mut state = populated();
        for command in commands {
            reduce(&mut state, Action::Command(command));
            for row in &state.tree.visible {
                prop_assert!(state.tree.model.contains(row.node));
                let mut node = row.node;
                let mut depth = 0u16;
                while let Some(parent) = state.tree.model.get(node).and_then(|e| e.parent) {
                    depth += 1;
                    node = parent;
                }
                prop_assert_eq!(depth, row.depth, "wrong depth for {:?}", row.node);
            }
        }
    }

    /// The selected row is on screen and is a row that exists.
    ///
    /// A hidden tree has no rows on screen, so the property only applies while
    /// the pane is there; hiding it is checked on its own further down.
    #[test]
    fn the_selection_stays_on_screen(
        commands in prop::collection::vec(tree_command(), 1..24)
    ) {
        let mut state = populated();
        for command in commands {
            reduce(&mut state, Action::Command(command));
            if !state.tree_visible {
                continue;
            }
            let Some(index) = state.tree.selected_index() else {
                prop_assert!(state.tree.selected.is_none());
                continue;
            };
            prop_assert!(state.tree.model.contains(state.tree.selected.unwrap()));
            let height = state.tree.viewport.max(1);
            prop_assert!(
                index >= state.tree.scroll && index < state.tree.scroll + height,
                "row {index} is outside {}..{}",
                state.tree.scroll,
                state.tree.scroll + height
            );
            prop_assert!(state.tree.scroll <= state.tree.max_scroll());
        }
    }

    /// Moving down at the end and up at the start leaves the selection alone.
    #[test]
    fn the_selection_stops_at_both_ends(down in 0usize..40, up in 0usize..40) {
        let mut state = populated();
        let last = state.tree.row_count() - 1;
        reduce(&mut state, Action::Command(Command::Last));
        prop_assert_eq!(state.tree.selected_index(), Some(last));
        for _ in 0..down {
            reduce(&mut state, Action::Command(Command::MoveDown));
        }
        prop_assert_eq!(state.tree.selected_index(), Some(last));

        let mut state = populated();
        reduce(&mut state, Action::Command(Command::First));
        prop_assert_eq!(state.tree.selected_index(), Some(0));
        for _ in 0..up {
            reduce(&mut state, Action::Command(Command::MoveUp));
        }
        prop_assert_eq!(state.tree.selected_index(), Some(0));
    }

    /// Every load that is asked for is registered, carries the generation the
    /// reducer knows about, and is for a directory.
    #[test]
    fn a_load_is_only_asked_for_a_directory(
        commands in prop::collection::vec(tree_command(), 1..16)
    ) {
        let mut state = populated();
        for command in commands {
            for effect in reduce(&mut state, Action::Command(command)) {
                let Effect::LoadDir { node, generation, .. } = effect else {
                    continue;
                };
                prop_assert!(
                    state.tree.model.get(node).expect("node").is_expandable(),
                    "a listing was asked for something that is not a directory"
                );
                prop_assert!(
                    state.tree.loading.contains(&node),
                    "a load was asked for without being registered"
                );
                prop_assert_eq!(
                    state.tree.load_gen[&node], generation,
                    "the effect carries a generation the reducer does not know"
                );
            }
        }
    }

    /// Collapsing a directory takes every descendant off the screen at once.
    #[test]
    fn collapsing_hides_the_descendants(
        commands in prop::collection::vec(tree_command(), 1..16),
        before in 0usize..12,
    ) {
        let mut state = populated();
        // Walk down the tree a little so that a nested directory gets expanded.
        for step in 0..before {
            reduce(
                &mut state,
                Action::Command(if step % 2 == 0 {
                    Command::MoveDown
                } else {
                    Command::ToggleExpand
                }),
            );
        }
        let expanded: Vec<NodeId> = state.tree.expanded.iter().copied().collect();
        prop_assume!(!expanded.is_empty());
        // Collapse one of the expanded directories, whichever is selected now.
        let collapsed = expanded[0];
        state.tree.select(collapsed);

        reduce(&mut state, Action::Command(Command::Collapse));
        let after: HashSet<NodeId> = state.tree.visible.iter().map(|row| row.node).collect();
        for row in &after {
            let mut node = *row;
            let mut descends = false;
            while let Some(parent) = state.tree.model.get(node).and_then(|e| e.parent) {
                if parent == collapsed {
                    descends = true;
                    break;
                }
                node = parent;
            }
            prop_assert!(!descends, "a descendant of the collapsed directory is drawn");
        }

        // Whatever comes next, the cache stays in step with the model.
        for command in commands {
            reduce(&mut state, Action::Command(command));
        }
        state.tree.refresh();
        let rows: HashSet<NodeId> = state.tree.visible.iter().map(|row| row.node).collect();
        prop_assert_eq!(rows, reachable_rows(&state));
    }
}

#[test]
fn a_listing_from_an_unknown_generation_is_dropped() {
    let mut state = populated();
    let root = state.tree.model.root_id();
    let docs = state
        .tree
        .model
        .child_id(root, std::ffi::OsStr::new("docs"))
        .expect("the fixture has a directory");
    state.tree.select(docs);

    let asked = reduce(&mut state, Action::Command(Command::Expand));
    assert!(
        matches!(asked.as_slice(), [Effect::LoadDir { .. }]),
        "{asked:?}"
    );
    let generation = state.tree.load_gen[&docs];
    let rows_before = state.tree.visible.clone();

    // A listing from a generation the reducer never handed out is not ours.
    reduce(
        &mut state,
        Action::DirLoaded {
            node: docs,
            generation: generation + 5,
            result: Ok(fixture_entries()),
        },
    );
    assert_eq!(
        state.tree.visible, rows_before,
        "a listing from an unknown generation changed the tree"
    );

    // The one that was asked for is the one that counts.
    reduce(
        &mut state,
        Action::DirLoaded {
            node: docs,
            generation,
            result: Ok(fixture_entries()),
        },
    );
    assert!(state.tree.visible.len() > rows_before.len());
}

#[test]
fn asking_while_a_load_is_in_flight_asks_once() {
    let mut state = populated();
    let root = state.tree.model.root_id();
    let docs = state
        .tree
        .model
        .child_id(root, std::ffi::OsStr::new("docs"))
        .expect("the fixture has a directory");
    state.tree.select(docs);

    let first = reduce(&mut state, Action::Command(Command::Expand));
    assert!(
        matches!(first.as_slice(), [Effect::LoadDir { .. }]),
        "{first:?}"
    );
    let generation = state.tree.load_gen[&docs];
    assert!(state.tree.loading.contains(&docs));

    // Reopening before the listing has arrived must not queue a second one.
    reduce(&mut state, Action::Command(Command::Collapse));
    let second = reduce(&mut state, Action::Command(Command::Expand));
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(state.tree.load_gen[&docs], generation);

    // Once it has arrived the directory is not read again.
    reduce(
        &mut state,
        Action::DirLoaded {
            node: docs,
            generation,
            result: Ok(fixture_entries()),
        },
    );
    assert!(!state.tree.loading.contains(&docs));
    reduce(&mut state, Action::Command(Command::Collapse));
    let third = reduce(&mut state, Action::Command(Command::Expand));
    assert!(
        third.is_empty(),
        "a directory that is already in hand: {third:?}"
    );
}

#[test]
fn hiding_the_tree_leaves_nothing_on_screen() {
    let mut state = support::state_with_entries(40);
    state.tree.set_viewport(21);
    reduce(&mut state, Action::Command(Command::Last));
    assert!(state.tree.scroll > 0, "the tree is scrolled down");

    reduce(&mut state, Action::Command(Command::ToggleTree));
    assert!(!state.tree_visible);
    assert_eq!(state.tree.viewport, 0);
    assert_eq!(state.tree.scroll, 0);
    // The rows are still there, they are simply not drawn.
    assert!(state.tree.row_count() > 21);

    reduce(&mut state, Action::Command(Command::ToggleTree));
    assert!(state.tree_visible);
    assert_eq!(state.tree.viewport, 21);
}

#[test]
fn expanding_a_file_asks_for_nothing() {
    let mut state = populated();
    let root = state.tree.model.root_id();
    let file = state
        .tree
        .model
        .child_id(root, std::ffi::OsStr::new("README.md"))
        .expect("the fixture has a file");
    state.tree.select(file);
    reduce(&mut state, Action::Command(Command::Last));

    let effects = reduce(&mut state, Action::Command(Command::Expand));
    assert!(
        effects.is_empty(),
        "a file must not be asked for a listing: {effects:?}"
    );
    assert!(!state.tree.loading.contains(&file));
    assert!(!state.tree.is_expanded(file));
}

#[test]
fn a_directory_that_cannot_be_read_stays_in_the_tree() {
    let mut state = populated();
    let root = state.tree.model.root_id();
    let generation = state.tree.load_gen[&root];
    reduce(
        &mut state,
        Action::DirLoaded {
            node: root,
            generation,
            result: Err(DirError::Io(std::io::Error::other("no such directory"))),
        },
    );
    let root_entry = state.tree.model.get(root).expect("root");
    assert!(root_entry.unreadable);
    assert!(state.tree.model.len() > 1, "the children are still listed");
    assert!(state.message.is_some(), "the user is told about it");
}

#[test]
fn a_listing_for_a_node_that_is_gone_is_ignored() {
    let mut state = populated();
    let rows = state.tree.visible.clone();
    // The null key is never a valid handle.
    let stranger = NodeId::default();
    assert!(!state.tree.model.contains(stranger));

    reduce(
        &mut state,
        Action::DirLoaded {
            node: stranger,
            generation: 1,
            result: Ok(fixture_entries()),
        },
    );
    assert_eq!(state.tree.visible, rows);
}

#[test]
fn a_selection_asks_the_preview_for_a_file_and_for_nothing_else() {
    // The reducer tests drive the preview reducer directly; this one goes
    // through the root reducer, where the tree and the preview meet.
    let mut state = populated();
    let root = state.tree.model.root_id();
    let readme = state
        .tree
        .model
        .child_id(root, std::ffi::OsStr::new("README.md"))
        .expect("the fixture has a file");
    let docs = state
        .tree
        .model
        .child_id(root, std::ffi::OsStr::new("docs"))
        .expect("the fixture has a directory");

    // Selecting the file and then pressing a key that does not move the
    // selection is enough to ask for it: the preview follows the tree.
    state.tree.select(readme);
    let effects = reduce(&mut state, Action::Command(Command::ToggleFocus));
    let loads: Vec<_> = effects
        .iter()
        .filter(|effect| matches!(effect, Effect::LoadPreview { .. }))
        .collect();
    assert_eq!(loads.len(), 1, "the file is previewed: {effects:?}");
    let Effect::LoadPreview {
        rel, generation, ..
    } = loads[0]
    else {
        unreachable!("filtered above");
    };
    assert_eq!(rel, &std::path::Path::new("README.md"));
    assert_eq!(*generation, state.preview.generation);

    // Moving on to a directory asks for nothing and empties the pane.
    state.tree.select(docs);
    let effects = reduce(&mut state, Action::Command(Command::ToggleFocus));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::LoadPreview { .. })),
        "a directory is not previewed: {effects:?}"
    );
    assert_eq!(state.preview.preview, PreviewStatus::Empty);

    // The result of the file that is no longer selected must not land.
    reduce(
        &mut state,
        Action::PreviewLoaded {
            rel: rel.clone(),
            generation: *generation,
            result: Ok(Doc {
                rel: rel.clone(),
                name: std::ffi::OsString::from("README.md"),
                size: 4,
                language: Language::Text,
                lines: vec!["late".to_owned()],
                truncated: false,
                too_large: false,
                spans: Vec::new(),
                highlight_truncated: false,
                invalid_utf8: false,
                binary: None,
                link_target: None,
                diff: None,
                diff_note: None,
            }),
        },
    );
    assert!(
        state.preview.doc.is_none(),
        "a stale document reached the pane"
    );
    assert_eq!(state.preview.preview, PreviewStatus::Empty);
}
