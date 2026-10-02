//! Drawing the tree pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::AppState;
use crate::core::baseline::Phase;
use crate::core::fsmodel::{Node, NodeKind, NodeStat, Status};
use crate::core::text;
use crate::core::text::sanitize_os;
use crate::features::tree::icons::{COLLAPSED, EXPANDED, LEAF};
use crate::ui::theme::Role;
use crate::ui::{pane, placeholder};

use crate::core::fsmodel::NodeId;

/// Draws the tree: borders, indentation, expand markers and icons.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(pane(&tree_title(state), &state.theme), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if state.tree.row_count() <= 1 && state.tree.model.len() <= 1 {
        placeholder(frame, inner, "loading…", &state.theme);
        return;
    }

    let height = inner.height as usize;
    let width = inner.width as usize;
    // Only the rows that fit are turned into lines, so a directory with a
    // hundred thousand entries costs the same as a small one.
    let rows = &state.tree.visible[state.tree.scroll.min(state.tree.visible.len())..];
    let lines: Vec<Line<'_>> = rows
        .iter()
        .take(height)
        .filter_map(|row| row_line(state, row.node, row.depth, width))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn tree_title(state: &AppState) -> String {
    state
        .root()
        .file_name()
        .map(sanitize_os)
        .unwrap_or_else(|| "/".to_owned())
}

/// One row, or `None` when the node vanished before the frame was drawn.
fn row_line<'a>(state: &'a AppState, node: NodeId, depth: u16, width: usize) -> Option<Line<'a>> {
    let entry = state.tree.model.get(node)?;
    let selected = state.tree.selected == Some(node);
    let base = state.theme.style(base_role(entry));
    let marker_style = state.theme.style(Role::PaneBorder);
    let name = sanitize_os(&entry.name);

    let mut spans = vec![
        Span::styled("  ".repeat(depth as usize), base),
        Span::styled(expander(state, node, entry), marker_style),
        Span::styled(format!("{} ", state.icons.for_kind(entry.kind)), base),
        Span::styled(name.clone(), base),
    ];
    if let Some(target) = &entry.link_target {
        spans.push(Span::styled(
            format!(" -> {}", sanitize_os(target)),
            state.theme.style(Role::TreeSymlink),
        ));
    }
    if entry.status != Status::Unchanged {
        spans.push(Span::styled(
            format!(" {}", entry.status.letter()),
            state.theme.style(Role::for_status(entry.status)),
        ));
    }
    if entry.unreadable {
        spans.push(Span::styled(" !", state.theme.style(Role::TreeUnreadable)));
    }
    if entry.skeleton {
        spans.push(Span::styled(
            " …",
            state.theme.style(Role::PreviewPlaceholder),
        ));
    }
    if state.tree.loading.contains(&node) {
        spans.push(Span::styled(
            " …",
            state.theme.style(Role::PreviewPlaceholder),
        ));
    }
    // The statistics sit at the right end of the row. The name is what
    // yields columns for them, and the part of it that tells names apart is
    // its end, so the cut is made at the front (§11.1).
    let stat = stat_cell(state, entry);
    let stat_width = text::display_width(&stat);
    let gap = usize::from(!stat.is_empty());
    let name_width = text::display_width(&name);
    let others_width: usize = spans
        .iter()
        .map(|span| text::display_width(span.content.as_ref()))
        .sum::<usize>()
        - name_width;
    if others_width + stat_width + gap > width {
        // Even the ellipsis has no room left; the pane clips what remains.
        spans[3].content = String::new().into();
    } else {
        // One column of the room goes to the ellipsis of a shortened name.
        let room = width.saturating_sub(others_width + stat_width + gap + 1);
        if name_width > room {
            spans[3].content = text::truncate_tail_to_width(&name, room).into();
        }
    }
    if !stat.is_empty() {
        let filled: usize = spans
            .iter()
            .map(|span| text::display_width(span.content.as_ref()))
            .sum();
        let pad = width.saturating_sub(filled + stat_width);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), base));
        }
        spans.push(Span::styled(stat, state.theme.style(Role::StatusMuted)));
    }
    if selected {
        // The selection is a background, so its modifiers are added to every
        // span of the row instead of replacing the span styles.
        let highlight = state.theme.style(Role::TreeSelected).add_modifier;
        for span in &mut spans {
            span.style = span.style.add_modifier(highlight);
        }
    }
    Some(Line::from(spans))
}

/// What the right end of a row says about the node's statistics (§11.1).
fn stat_cell(state: &AppState, entry: &Node) -> String {
    match entry.stat {
        NodeStat::Known(stat) => stat.format(),
        NodeStat::NotApplicable => String::new(),
        NodeStat::Unknown => match state.baseline.phase {
            // The baseline is being counted right now, and a directory may
            // still get a total.
            Phase::Loading if entry.kind.is_dir() => "\u{2026}".to_owned(),
            Phase::Loading | Phase::Unavailable => String::new(),
            // The count is done but this node has no number: its files are
            // not text, or are past the budget.
            Phase::Ready => "?".to_owned(),
        },
    }
}

/// The marker in front of the name.
fn expander(state: &AppState, node: NodeId, entry: &Node) -> &'static str {
    if !entry.is_expandable() {
        return LEAF;
    }
    if state.tree.is_expanded(node) {
        EXPANDED
    } else {
        COLLAPSED
    }
}

fn base_role(entry: &Node) -> Role {
    if entry.unreadable {
        return Role::TreeUnreadable;
    }
    match entry.kind {
        NodeKind::Dir => Role::TreeDirectory,
        NodeKind::Symlink => Role::TreeSymlink,
        NodeKind::File | NodeKind::Other => Role::TreeText,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::state::{Config, Focus};
    use crate::cli::{BaselineKind, IconStyle};
    use crate::core::fsmodel::{DirEntry, FsModel};
    use crate::features::tree::state::TreeState;
    use crate::ui::layout;

    fn config(no_color: bool) -> Config {
        Config {
            root: PathBuf::from("/root"),
            icons: IconStyle::Text,
            baseline: BaselineKind::Auto,
            ignore_enabled: true,
            no_color,
            log: None,
            selftest_panic: false,
        }
    }

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

    fn state_with_tree() -> AppState {
        let mut state = AppState::new(config(false), (80, 24));
        let mut model = FsModel::new(PathBuf::from("/root"));
        let root = model.root_id();
        let children = model.merge_children(
            root,
            &entries(&[
                ("src", NodeKind::Dir),
                ("readme.md", NodeKind::File),
                ("link", NodeKind::Symlink),
            ]),
        );
        model.get_mut(children[2]).expect("link").link_target =
            Some(OsStr::new("elsewhere").to_os_string());
        model.merge_children(children[0], &entries(&[("main.rs", NodeKind::File)]));
        state.tree = TreeState::new(model);
        state.tree.expand(state.tree.model.root_id());
        state
            .tree
            .expand(state.tree.model.lookup(Path::new("src")).expect("src"));
        state.tree.set_viewport(state.tree_viewport_height());
        state.tree.bump();
        state.tree.refresh();
        state
    }

    fn draw(state: &AppState) -> String {
        let area = layout::compute(state.area(), state.tree_visible).tree;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, area, state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    #[test]
    fn rows_show_indent_marker_icon_and_name() {
        let text = draw(&state_with_tree());
        assert!(text.contains("root"), "{text}");
        assert!(text.contains('\u{25be}'), "{text}");
        assert!(text.contains("d src"), "{text}");
        assert!(text.contains("f main.rs"), "{text}");
        assert!(text.contains("f readme.md"), "{text}");
    }

    #[test]
    fn expanded_and_collapsed_directories_use_different_markers() {
        let mut state = state_with_tree();
        assert!(draw(&state).contains('\u{25be}'));
        state.tree.collapse(state.tree.model.root_id());
        state.tree.refresh();
        assert!(draw(&state).contains('\u{25b8}'));
    }

    #[test]
    fn symlinks_show_their_target() {
        let text = draw(&state_with_tree());
        assert!(text.contains("l link -> elsewhere"), "{text}");
    }

    #[test]
    fn file_names_never_reach_the_terminal_unescaped() {
        let mut state = state_with_tree();
        let root = state.tree.model.root_id();
        state
            .tree
            .model
            .merge_children(root, &entries(&[("evil\u{1b}[2J.rs", NodeKind::File)]));
        state.tree.bump();
        state.tree.refresh();
        let text = draw(&state);
        assert!(text.contains('\u{241b}'), "{text}");
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn the_selected_row_is_reversed() {
        let state = state_with_tree();
        let area = layout::compute(state.area(), true).tree;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, area, &state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let row = state.tree.selected_index().expect("selected") + 1;
        let cell = &buffer[(1, row as u16)];
        assert!(
            cell.modifier.contains(ratatui::style::Modifier::REVERSED),
            "selected row was not reversed"
        );
        assert_eq!(state.focus, Focus::Tree);
    }

    #[test]
    fn an_empty_tree_shows_a_placeholder() {
        let state = AppState::new(config(false), (80, 24));
        let text = draw(&state);
        assert!(text.contains("loading"), "{text}");
    }

    #[test]
    fn a_zero_area_draws_nothing() {
        let state = state_with_tree();
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).expect("terminal");
        terminal
            .draw(|frame| render(frame, Rect::ZERO, &state))
            .expect("draw");
    }

    /// The tree pane's inner row mentioning `needle`, without borders.
    fn row_of(state: &AppState, needle: &str) -> String {
        let area = layout::compute(state.area(), state.tree_visible).tree;
        let inner = area.inner(ratatui::layout::Margin::new(1, 1));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, area, state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (inner.top()..inner.bottom())
            .map(|y| {
                (inner.left()..inner.right())
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no row of {needle:?}"))
    }

    #[test]
    fn the_stat_column_says_how_a_row_is_counted() {
        use crate::core::diff::DiffStat;

        let mut state = state_with_tree();
        assert!(
            row_of(&state, "d src").trim_end().ends_with('\u{2026}'),
            "a directory counts while the baseline builds"
        );
        assert!(
            row_of(&state, "f main.rs").trim_end().ends_with("main.rs"),
            "a file has no number before the count"
        );

        state.baseline.phase = Phase::Ready;
        let src = state.tree.model.lookup(Path::new("src")).expect("src");
        state.tree.model.get_mut(src).expect("src").stat = NodeStat::Known(DiffStat {
            added: 2,
            removed: 1,
            replaced: 0,
        });
        let readme = state
            .tree
            .model
            .lookup(Path::new("readme.md"))
            .expect("readme");
        state.tree.model.get_mut(readme).expect("readme").stat = NodeStat::Unknown;
        let main = state
            .tree
            .model
            .lookup(Path::new("src/main.rs"))
            .expect("main");
        state.tree.model.get_mut(main).expect("main").stat = NodeStat::NotApplicable;

        let text = draw(&state);
        assert!(
            row_of(&state, "d src").trim_end().ends_with("+2 -1"),
            "the collapsed rows were not summed up: {text}"
        );
        assert!(
            row_of(&state, "f readme.md").trim_end().ends_with('?'),
            "a file with no number says so: {text}"
        );
        assert!(
            row_of(&state, "f main.rs").trim_end().ends_with("main.rs"),
            "a file that cannot differ shows no cell: {text}"
        );
    }

    #[test]
    fn a_long_name_gives_up_its_front_for_the_statistics() {
        use crate::core::diff::DiffStat;

        let mut state = state_with_tree();
        state.baseline.phase = Phase::Ready;
        let src = state.tree.model.lookup(Path::new("src")).expect("src");
        let long = "a_very_long_file_name_that_will_never_fit_into_a_tree_row.rs";
        let file = state
            .tree
            .model
            .insert_child(src, OsStr::new(long), NodeKind::File);
        state.tree.model.get_mut(file).expect("file").stat = NodeStat::Known(DiffStat {
            added: 1,
            removed: 0,
            replaced: 0,
        });
        state.tree.bump();
        state.tree.refresh();

        let row = row_of(&state, "+1");
        assert!(row.trim_end().ends_with("+1"), "{row:?}");
        assert!(
            row.contains('\u{2026}'),
            "the name was not cut to make room: {row:?}"
        );
        assert!(!row.contains(long), "the whole name fit after all: {row:?}");
    }
}
