//! Drawing the tree pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::{AppState, Focus};
use crate::core::baseline::Phase;
use crate::core::fsmodel::{Node, NodeKind, NodeStat, Status};
use crate::core::text;
use crate::core::text::sanitize_os;
use crate::features::tree::icons::{COLLAPSED, EXPANDED, LEAF};
use crate::ui::theme::Role;
use crate::ui::{elapsed, pane, placeholder};

use crate::core::fsmodel::NodeId;

/// Draws the tree: borders, indentation, expand markers and icons.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        pane(&tree_title(state), &state.theme, state.focus == Focus::Tree),
        area,
    );
    // Two cells: the border, then a cell of air, so no row touches the
    // frame and the selection stops short of it on both sides.
    let inner = area.inner(ratatui::layout::Margin::new(2, 1));
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
        // The marker keeps a cell to its right, so the icon never sits
        // against it.
        Span::styled(format!("{} ", expander(state, node, entry)), marker_style),
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
    // The statistics and the age sit at the right end of the row. The name
    // is what yields columns for them, and the part of it that tells names
    // apart is its end, so the cut is made at the front (§11.1).
    let stat = stat_cell(state, node, entry);
    let elapsed = elapsed_cell(state, node, entry);
    let separator = if stat.is_empty() || elapsed.is_empty() {
        ""
    } else {
        " "
    };
    let right_width = text::display_width(&stat)
        + text::display_width(&elapsed)
        + usize::from(!separator.is_empty());
    let gap = usize::from(right_width > 0);
    let name_width = text::display_width(&name);
    let others_width: usize = spans
        .iter()
        .map(|span| text::display_width(span.content.as_ref()))
        .sum::<usize>()
        - name_width;
    if others_width + right_width + gap > width {
        // Even the ellipsis has no room left; the pane clips what remains.
        spans[3].content = String::new().into();
    } else {
        // One column of the room goes to the ellipsis of a shortened name.
        let room = width.saturating_sub(others_width + right_width + gap + 1);
        if name_width > room {
            spans[3].content = text::truncate_tail_to_width(&name, room).into();
        }
    }
    // Every row is filled out to the pane's width, whether or not it has
    // anything to say at its right end, so that the selection's background
    // reaches the border.
    let filled: usize = spans
        .iter()
        .map(|span| text::display_width(span.content.as_ref()))
        .sum();
    let pad = width.saturating_sub(filled + right_width);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), base));
    }
    if !stat.is_empty() {
        spans.push(Span::styled(stat, state.theme.style(Role::StatusMuted)));
    }
    if !elapsed.is_empty() {
        let style = state.theme.style(elapsed::role(elapsed::freshness(
            entry.changed_at.expect("an age to draw"),
            state.now,
        )));
        spans.push(Span::styled(format!("{separator}{elapsed}"), style));
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
///
/// A directory keeps the totals of the rows it hides; once it is open, those
/// rows say it themselves (spec, tree rows).
fn stat_cell(state: &AppState, node: NodeId, entry: &Node) -> String {
    if entry.is_expandable() && state.tree.is_expanded(node) {
        return String::new();
    }
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

/// How long ago the node was last seen to change (spec, tree rows).
///
/// A directory carries the newest age below it, and hides it while it is
/// open for the same reason as its statistics.
fn elapsed_cell(state: &AppState, node: NodeId, entry: &Node) -> String {
    if entry.is_expandable() && state.tree.is_expanded(node) {
        return String::new();
    }
    entry
        .changed_at
        .map(|changed_at| elapsed::label(changed_at, state.now))
        .unwrap_or_default()
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
    use std::time::{Duration, SystemTime};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};

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
    fn a_deleted_file_keeps_its_row_and_says_d_beside_the_name() {
        let mut state = state_with_tree();
        let root = state.tree.model.root_id();
        let gone = state
            .tree
            .model
            .merge_children(root, &entries(&[("gone.txt", NodeKind::File)]));
        state.tree.model.get_mut(gone[0]).expect("gone").status = Status::Deleted;
        state.tree.bump();
        state.tree.refresh();
        let text = draw(&state);
        assert!(text.contains("f gone.txt D"), "{text}");
    }

    #[test]
    fn a_directory_that_cannot_be_read_carries_a_bang() {
        let mut state = state_with_tree();
        let root = state.tree.model.root_id();
        let locked = state
            .tree
            .model
            .merge_children(root, &entries(&[("locked", NodeKind::Dir)]));
        state
            .tree
            .model
            .get_mut(locked[0])
            .expect("locked")
            .unreadable = true;
        state.tree.bump();
        state.tree.refresh();
        let text = draw(&state);
        assert!(text.contains("d locked !"), "{text}");
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
        // The first content column: one cell of air, then the border.
        let cell = &buffer[(2, row as u16)];
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
    fn the_selection_reaches_the_end_of_the_row() {
        let state = state_with_tree();
        let area = layout::compute(state.area(), state.tree_visible).tree;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, area, &state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        // The root has nothing to draw at its right end while it is open,
        // and the bar must still run to the cell the air leaves before the
        // border.
        let row = state.tree.selected_index().expect("selected") + 1;
        let right = area.left() + area.width - 3;
        let cell = &buffer[(right, row as u16)];
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "the selection stopped short of the border: {cell:?}"
        );
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
        let inner = area.inner(ratatui::layout::Margin::new(2, 1));
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
        let src = state.tree.model.lookup(Path::new("src")).expect("src");
        state.tree.collapse(src);
        state.tree.refresh();
        assert!(
            row_of(&state, "d src").trim_end().ends_with('\u{2026}'),
            "a directory counts while the baseline builds"
        );
        state.tree.expand(src);
        state.tree.refresh();
        assert!(
            row_of(&state, "f main.rs").trim_end().ends_with("main.rs"),
            "a file has no number before the count"
        );

        state.baseline.phase = Phase::Ready;
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
            row_of(&state, "f readme.md").trim_end().ends_with('?'),
            "a file with no number says so: {text}"
        );
        assert!(
            row_of(&state, "f main.rs").trim_end().ends_with("main.rs"),
            "a file that cannot differ shows no cell: {text}"
        );
        // The rows below an open directory say it themselves, so the
        // directory's own totals are drawn only while it hides them.
        state.tree.collapse(src);
        state.tree.refresh();
        let hidden = draw(&state);
        assert!(
            row_of(&state, "d src").trim_end().ends_with("+2 -1"),
            "the collapsed rows were not summed up: {hidden}"
        );
        state.tree.expand(src);
        state.tree.refresh();
        let open = row_of(&state, "d src");
        assert!(
            open.trim_end().ends_with("src"),
            "an open directory kept a cell of its own: {open:?}"
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

    /// Puts a change on `path`, `ago` before the clock the state reads.
    fn changed_at(state: &mut AppState, path: &str, ago: Duration) {
        let id = state.tree.model.lookup(Path::new(path)).expect(path);
        let now = state.now;
        state.tree.model.get_mut(id).expect(path).changed_at = Some(now - ago);
    }

    /// A clock far enough ahead that any age can be counted back from it.
    fn set_clock(state: &mut AppState) {
        state.now = SystemTime::UNIX_EPOCH + Duration::from_secs(30 * 24 * 60 * 60);
    }

    /// The style the word `word` is drawn in, somewhere in the tree pane.
    fn word_style(state: &AppState, word: &str) -> (Color, Modifier) {
        let area = layout::compute(state.area(), state.tree_visible).tree;
        let inner = area.inner(ratatui::layout::Margin::new(2, 1));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| render(frame, area, state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        for y in inner.top()..inner.bottom() {
            let line: String = (inner.left()..inner.right())
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect();
            if let Some(byte) = line.find(word) {
                // The pane draws one cell per character, so the column is a
                // character count and not a byte offset.
                let column = line[..byte].chars().count();
                let cell = &buffer[(inner.left() + column as u16, y)];
                return (cell.fg, cell.modifier);
            }
        }
        panic!("no {word:?} drawn in the tree pane");
    }

    #[test]
    fn a_row_that_changed_says_how_long_ago_and_a_plain_one_does_not() {
        let mut state = state_with_tree();
        set_clock(&mut state);
        changed_at(&mut state, "readme.md", Duration::from_secs(12));

        let row = row_of(&state, "f readme.md");
        assert!(row.trim_end().ends_with("12s"), "{row:?}");
        assert!(
            row_of(&state, "f main.rs").trim_end().ends_with("main.rs"),
            "a row nothing happened to said an age"
        );
    }

    #[test]
    fn the_age_of_a_change_is_drawn_in_its_freshness() {
        let mut state = state_with_tree();
        set_clock(&mut state);
        changed_at(&mut state, "readme.md", Duration::from_secs(9));
        changed_at(&mut state, "src/main.rs", Duration::from_secs(5 * 60));
        changed_at(&mut state, "link", Duration::from_secs(5 * 24 * 60 * 60));

        let (fresh_color, fresh_mods) = word_style(&state, "9s");
        assert_eq!(fresh_color, Color::Yellow);
        assert!(fresh_mods.contains(Modifier::BOLD), "{fresh_mods:?}");

        let (recent_color, recent_mods) = word_style(&state, "5m");
        assert_eq!(recent_color, Color::Reset);
        assert!(!recent_mods.intersects(Modifier::BOLD | Modifier::DIM));

        let (stale_color, stale_mods) = word_style(&state, "5d");
        assert_eq!(stale_color, Color::DarkGray);
        assert!(stale_mods.contains(Modifier::DIM), "{stale_mods:?}");
    }

    #[test]
    fn a_directory_keeps_its_age_until_it_is_opened() {
        let mut state = state_with_tree();
        set_clock(&mut state);
        changed_at(&mut state, "src", Duration::from_secs(42));

        state
            .tree
            .collapse(state.tree.model.lookup(Path::new("src")).expect("src"));
        state.tree.refresh();
        let hidden = row_of(&state, "d src");
        assert!(hidden.trim_end().ends_with("42s"), "{hidden:?}");

        state
            .tree
            .expand(state.tree.model.lookup(Path::new("src")).expect("src"));
        state.tree.refresh();
        let open = row_of(&state, "d src");
        assert!(
            open.trim_end().ends_with("src"),
            "an open directory kept the age of what it hides: {open:?}"
        );
    }
}
