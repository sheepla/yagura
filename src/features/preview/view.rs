//! The preview header and body.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::state::{AppState, PreviewMode};
use crate::core::diff::Row;
use crate::core::fsmodel::{Node, Status};
use crate::core::highlight::{self, Semantic};
use crate::core::preview::{Doc, human_size};
use crate::core::text;
use crate::core::text::sanitize_os;
use crate::ui::theme::{Role, Theme};
use crate::ui::{pane_line, placeholder};

use super::state::PreviewStatus;

/// Columns the line numbers take in front of every line.
pub const GUTTER_COLS: usize = 5;

/// Columns the terminal needs before the split view is drawn. Below it the
/// two halves are too narrow to read, so the inline diff stands in for it
/// and the status bar says why (§9.2).
pub const SPLIT_MIN_COLS: u16 = 100;

/// Draws the file header above the body.
pub fn render_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // A loaded document knows more about itself than a path does: it can add the
    // binary kind, the size and the symlink target.
    let title = match state.preview.doc.as_deref() {
        Some(doc) => doc.header(),
        None => match state.preview.path.as_ref() {
            Some(path) => sanitize_os(path.file_name().unwrap_or(path.as_os_str())),
            None => String::new(),
        },
    };
    // The icon and the status come from the tree's node, which is the one
    // place both of them are known (§6).
    let node = shown_node(state);
    let mut spans = Vec::new();
    if let Some(node) = node {
        spans.push(Span::styled(
            format!("{} ", state.icons.for_kind(node.kind)),
            state.theme.style(Role::PaneTitle),
        ));
    }
    spans.push(Span::styled(title, state.theme.style(Role::PaneTitle)));
    if let Some(node) = node
        && node.status != Status::Unchanged
    {
        spans.push(Span::styled(
            format!("  {}", node.status.label()),
            state.theme.style(Role::for_status(node.status)),
        ));
    }
    frame.render_widget(pane_line(Line::from(spans), &state.theme), area);
}

/// The tree node of the file the preview points at, when there is one.
fn shown_node(state: &AppState) -> Option<&Node> {
    state
        .preview
        .path
        .as_deref()
        .and_then(|path| state.tree.model.lookup(path))
        .and_then(|id| state.tree.model.get(id))
}

/// Draws the body: the document, or why there is none.
pub fn render_body(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if state.preview.path.is_none() {
        placeholder(frame, area, "no file selected", &state.theme);
        return;
    }
    let Some(doc) = state.preview.doc.as_deref() else {
        let hint = match state.preview.preview {
            PreviewStatus::Loading => "loading…",
            // A file the baseline still has speaks for itself (§6).
            PreviewStatus::Failed
                if shown_node(state).is_some_and(|node| node.status == Status::Deleted) =>
            {
                "file deleted"
            }
            PreviewStatus::Failed => "cannot read this file",
            _ => {
                if state.preview_mode.is_diff() {
                    "no changes"
                } else {
                    "no content"
                }
            }
        };
        line_of_hint(frame, area, hint, &state.theme);
        return;
    };
    if let Some(kind) = doc.binary {
        placeholder(
            frame,
            area,
            &format!("{} · {}", kind.label(), human_size(doc.size)),
            &state.theme,
        );
        return;
    }
    frame.render_widget(Paragraph::new(body(state, doc, area)), area);
}

/// The visible lines of the document, cut to the pane.
///
/// A diff mode with a diff draws its rows, in one column or two; anything
/// else draws the plain content. Only the rows the pane can show are built,
/// so a 200,000 line document costs the same as a 20 line one.
fn body<'a>(state: &'a AppState, doc: &'a Doc, area: Rect) -> Text<'a> {
    if state.preview_mode.is_diff()
        && let Some(diff) = &doc.diff
    {
        let split = state.preview_mode == PreviewMode::SideBySide && state.size.0 >= SPLIT_MIN_COLS;
        return if split {
            split_body(state, doc, &diff.rows, area)
        } else {
            inline_body(state, doc, &diff.rows, area)
        };
    }
    plain_body(state, doc, area)
}

/// The document as it is on disk, drawn with the meanings of each line.
fn plain_body<'a>(state: &'a AppState, doc: &'a Doc, area: Rect) -> Text<'a> {
    let height = state.preview.height();
    // The gutter takes five columns, and the pane keeps one column of margin.
    let width = area.width.saturating_sub(GUTTER_COLS as u16 + 1) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let end = doc
        .lines
        .len()
        .min(state.preview.scroll_line.saturating_add(height));
    for index in state.preview.scroll_line..end {
        let runs = doc.spans.get(index).map(Vec::as_slice).unwrap_or_default();
        let mut spans = vec![Span::styled(
            format!("{:>4} ", index + 1),
            state.theme.style(Role::PreviewGutter),
        )];
        spans.extend(cut(
            &doc.lines[index],
            runs,
            state.preview.scroll_col,
            width,
            &state.theme,
        ));
        lines.push(Line::from(spans));
    }
    while lines.len() < height {
        lines.push(Line::default());
    }
    Text::from(lines)
}

/// The diff as one column: every row keeps its number and gets a `+` or a
/// `-` in front of it, and the row is drawn in the colour of the side it
/// came from (§9.2).
fn inline_body<'a>(state: &'a AppState, doc: &'a Doc, rows: &'a [Row], area: Rect) -> Text<'a> {
    let height = state.preview.height();
    let width = area.width.saturating_sub(GUTTER_COLS as u16 + 1) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let end = doc
        .lines
        .len()
        .min(state.preview.scroll_line.saturating_add(height));
    for index in state.preview.scroll_line..end {
        let (marker, role) = marker_of(rows.get(index));
        let style = state.theme.style(role);
        let window = text::window(&doc.lines[index], state.preview.scroll_col, width);
        let mut spans = vec![
            Span::styled(
                format!("{:>4}", index + 1),
                state.theme.style(Role::PreviewGutter),
            ),
            Span::styled(marker.to_string(), style),
            Span::styled(&doc.lines[index][window.start..window.end], style),
        ];
        if window.cut {
            spans.push(Span::styled("…", style));
        }
        lines.push(Line::from(spans));
    }
    while lines.len() < height {
        lines.push(Line::default());
    }
    Text::from(lines)
}

/// The marker and the colour one diff row is drawn with.
fn marker_of(row: Option<&Row>) -> (char, Role) {
    match row {
        Some(Row::Added(_)) => ('+', Role::DiffAdded),
        Some(Row::Removed(_)) => ('-', Role::DiffRemoved),
        Some(Row::Fold(_)) => (' ', Role::PreviewPlaceholder),
        Some(Row::Context(_)) | None => (' ', Role::PreviewText),
    }
}

/// What one half of a split row shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell<'a> {
    /// The same text on both sides.
    Shared(&'a str),
    /// A collapsed run, spelled out on both sides.
    Fold(&'a str),
    /// A line only one side has.
    Changed(&'a str),
    /// Nothing on this side.
    Blank,
}

/// The diff as two columns: the baseline on the left, the file on the right
/// (§9.2). The rows are paired here rather than stored, so a mode switch
/// costs nothing.
fn split_body<'a>(state: &'a AppState, doc: &'a Doc, rows: &'a [Row], area: Rect) -> Text<'a> {
    let height = state.preview.height();
    // One column of the pane's margin becomes the divider between the sides.
    let left = area.width / 2;
    let right = area.width.saturating_sub(left + 1);
    let left_text = left.saturating_sub(GUTTER_COLS as u16) as usize;
    let right_text = right.saturating_sub(GUTTER_COLS as u16) as usize;
    let cells = side_cells(rows, &doc.lines);
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let start = state.preview.scroll_line;
    let end = cells.len().min(start.saturating_add(height));
    for (index, pair) in cells.iter().enumerate().take(end).skip(start) {
        let [baseline, current] = *pair;
        let mut spans = Vec::with_capacity(7);
        spans.extend(cell_spans(state, index, baseline, true, left_text));
        spans.push(Span::styled("│", state.theme.style(Role::PreviewGutter)));
        spans.extend(cell_spans(state, index, current, false, right_text));
        lines.push(Line::from(spans));
    }
    while lines.len() < height {
        lines.push(Line::default());
    }
    Text::from(lines)
}

/// One half of a split row: the number, the marker, the window of text, and
/// the padding that holds the other half in place.
fn cell_spans<'a>(
    state: &'a AppState,
    index: usize,
    cell: Cell<'a>,
    baseline_side: bool,
    width: usize,
) -> Vec<Span<'a>> {
    let gutter = Span::styled(
        format!("{:>4}", index + 1),
        state.theme.style(Role::PreviewGutter),
    );
    let (text, marker, role) = match (cell, baseline_side) {
        (Cell::Changed(text), true) => (text, '-', Role::DiffRemoved),
        (Cell::Changed(text), false) => (text, '+', Role::DiffAdded),
        (Cell::Fold(text), _) => (text, ' ', Role::PreviewPlaceholder),
        (Cell::Shared(text), _) => (text, ' ', Role::PreviewText),
        (Cell::Blank, _) => ("", ' ', Role::PreviewText),
    };
    let style = state.theme.style(role);
    if matches!(cell, Cell::Blank) {
        // The absent side still takes its width, or the other half shifts.
        return vec![
            gutter,
            Span::styled(marker.to_string(), style),
            Span::styled(" ".repeat(width), style),
        ];
    }
    let window = text::window(text, state.preview.scroll_col, width);
    let mut spans = vec![
        gutter,
        Span::styled(marker.to_string(), style),
        Span::styled(&text[window.start..window.end], style),
    ];
    if window.cut {
        spans.push(Span::styled("…", style));
    }
    let shown = text::display_width(&text[window.start..window.end]) + usize::from(window.cut);
    let padding = width.saturating_sub(shown);
    if padding > 0 {
        spans.push(Span::styled(
            " ".repeat(padding),
            state.theme.style(Role::PreviewText),
        ));
    }
    spans
}

/// Pairs the rows into the two columns: within a change block the two runs
/// are matched from the top, and whatever one side has too much of keeps its
/// own side, exactly as `Diff::side_rows` draws it.
fn side_cells<'a>(rows: &'a [Row], lines: &'a [String]) -> Vec<[Cell<'a>; 2]> {
    let mut cells = Vec::with_capacity(rows.len());
    let mut index = 0;
    while index < rows.len() {
        match &rows[index] {
            Row::Context(line) => {
                cells.push([Cell::Shared(line), Cell::Shared(line)]);
                index += 1;
            }
            Row::Fold(_) => {
                let text = lines.get(index).map_or("", String::as_str);
                cells.push([Cell::Fold(text), Cell::Fold(text)]);
                index += 1;
            }
            Row::Removed(_) | Row::Added(_) => {
                let start = index;
                while index < rows.len() && matches!(rows[index], Row::Removed(_) | Row::Added(_)) {
                    index += 1;
                }
                let mut removed: Vec<&str> = Vec::new();
                let mut added: Vec<&str> = Vec::new();
                for row in &rows[start..index] {
                    match row {
                        Row::Removed(line) => removed.push(line),
                        Row::Added(line) => added.push(line),
                        _ => {}
                    }
                }
                let paired = removed.len().min(added.len());
                for pair in 0..paired {
                    cells.push([Cell::Changed(removed[pair]), Cell::Changed(added[pair])]);
                }
                for line in &removed[paired..] {
                    cells.push([Cell::Changed(line), Cell::Blank]);
                }
                for line in &added[paired..] {
                    cells.push([Cell::Blank, Cell::Changed(line)]);
                }
            }
        }
    }
    cells
}

/// One line, cut to the pane and split by what each run of it means.
///
/// The gaps between runs mean nothing and are drawn as the pane's own text,
/// so a line the reader never highlighted still comes out whole.
fn cut<'a>(
    line: &'a str,
    runs: &'a [highlight::Span],
    from: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let window = crate::core::text::window(line, from, width);
    let base = theme.style(Role::PreviewText);
    let mut out = Vec::new();
    let mut cursor = window.start;
    for run in runs {
        if run.end <= window.start {
            continue;
        }
        if run.start >= window.end {
            break;
        }
        let start = run.start.max(window.start);
        let end = run.end.min(window.end);
        if start > cursor {
            out.push(Span::styled(&line[cursor..start], base));
        }
        if end > start {
            out.push(Span::styled(
                &line[start..end],
                theme.style(role(run.semantic)),
            ));
        }
        cursor = end;
    }
    if cursor < window.end {
        out.push(Span::styled(&line[cursor..window.end], base));
    }
    if window.cut {
        out.push(Span::styled("…", base));
    }
    out
}

/// The role a meaning is drawn with.
fn role(semantic: Semantic) -> Role {
    match semantic {
        Semantic::Plain => Role::PreviewText,
        Semantic::Keyword => Role::SyntaxKeyword,
        Semantic::String => Role::SyntaxString,
        Semantic::Comment => Role::SyntaxComment,
        Semantic::Number => Role::SyntaxNumber,
        Semantic::Type => Role::SyntaxType,
        Semantic::Function => Role::SyntaxFunction,
    }
}

fn line_of_hint(frame: &mut Frame<'_>, area: Rect, hint: &str, theme: &Theme) {
    let line = Line::from(vec![
        Span::styled(" ", theme.style(Role::PreviewText)),
        Span::styled(hint.to_owned(), theme.style(Role::PreviewPlaceholder)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::app::state::{AppState, PreviewMode};
    use crate::cli::{BaselineKind, Config, IconStyle};
    use crate::core::preview::{BinaryKind, Language};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};

    fn state() -> AppState {
        AppState::new(
            Config {
                root: PathBuf::from("/root"),
                icons: IconStyle::Text,
                baseline: BaselineKind::Auto,
                ignore_enabled: true,
                no_color: false,
                log: None,
                selftest_panic: false,
            },
            (80, 24),
        )
    }

    fn draw(state: &AppState) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).expect("terminal");
        terminal
            .draw(|frame| {
                render_header(frame, frame.area(), state);
                render_body(
                    frame,
                    frame.area().inner(ratatui::layout::Margin::new(0, 1)),
                    state,
                );
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn buffer(state: &AppState) -> String {
        let buffer = draw(state);
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
    fn the_body_says_when_nothing_is_selected() {
        let text = buffer(&state());
        assert!(text.contains("no file selected"));
    }

    #[test]
    fn the_header_shows_the_sanitized_file_name() {
        let mut state = state();
        state.preview.path = Some(PathBuf::from("/root/we\x1b[31mird.rs"));
        let text = buffer(&state);
        assert!(text.contains("we\u{241b}[31mird.rs"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn the_header_shows_the_icon_and_the_status_of_the_shown_file() {
        use crate::core::fsmodel::NodeKind;

        let mut state = state();
        let root = state.tree.model.root_id();
        let node =
            state
                .tree
                .model
                .insert_child(root, std::ffi::OsStr::new("a.rs"), NodeKind::File);
        state.preview.path = Some(PathBuf::from("a.rs"));
        let unchanged = buffer(&state);
        assert!(
            unchanged.contains("f a.rs"),
            "the icon of the shown file is missing: {unchanged:?}"
        );
        assert!(
            !unchanged.contains("Modified"),
            "an unchanged file wears a status: {unchanged:?}"
        );

        state.tree.model.get_mut(node).expect("node").status = Status::Modified;
        let modified = buffer(&state);
        assert!(modified.contains("f a.rs  Modified"), "{modified:?}");
    }

    #[test]
    fn a_failed_load_of_a_deleted_file_says_file_deleted() {
        use crate::core::fsmodel::NodeKind;

        let mut state = state();
        let root = state.tree.model.root_id();
        let node =
            state
                .tree
                .model
                .insert_child(root, std::ffi::OsStr::new("a.rs"), NodeKind::File);
        state.preview.path = Some(PathBuf::from("a.rs"));
        state.preview.set_failed();
        state.tree.model.get_mut(node).expect("node").status = Status::Deleted;
        assert!(buffer(&state).contains("file deleted"));

        state.tree.model.get_mut(node).expect("node").status = Status::Unchanged;
        assert!(buffer(&state).contains("cannot read this file"));
    }

    #[test]
    fn diff_modes_without_content_say_no_changes() {
        let mut state = state();
        state.preview.path = Some(PathBuf::from("/root/main.rs"));
        state.preview_mode = PreviewMode::InlineDiff;
        assert!(buffer(&state).contains("no changes"));
    }

    /// A state whose preview already shows `lines` lines of `a.rs`.
    fn loaded(lines: &[&str]) -> AppState {
        let mut state = state();
        state.preview.viewport = 4;
        let doc = Doc {
            rel: PathBuf::from("a.rs"),
            name: "a.rs".into(),
            size: 128,
            language: Language::Rust,
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            truncated: false,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        state
    }

    #[test]
    fn the_body_draws_the_lines_with_their_numbers() {
        let text = buffer(&loaded(&["fn main() {", "    run();", "}"]));
        assert!(text.contains("   1 fn main() {"), "{text}");
        assert!(text.contains("   2     run();"), "{text}");
        assert!(text.contains("   3 }"), "{text}");
    }

    #[test]
    fn the_body_only_draws_what_the_pane_can_hold() {
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let text = buffer(&loaded(&refs));
        assert!(text.contains("   1 line 1"), "{text}");
        assert!(text.contains("   4 line 4"), "{text}");
        assert!(!text.contains("line 5"), "drew past the pane:\n{text}");
    }

    #[test]
    fn scrolling_moves_the_window_over_the_document() {
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut state = loaded(&refs);
        state.preview.scroll_line = 10;
        let text = buffer(&state);
        assert!(text.contains("  11 line 11"), "{text}");
        assert!(!text.contains("line 10"), "{text}");
    }

    #[test]
    fn a_binary_document_shows_its_kind_and_size_instead_of_bytes() {
        let mut state = state();
        let mut doc = Doc {
            rel: PathBuf::from("logo.png"),
            name: "logo.png".into(),
            size: 4096,
            language: Language::Plain,
            lines: Vec::new(),
            truncated: false,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: Some(BinaryKind::Image),
            link_target: None,
            diff: None,
            diff_note: None,
        };
        doc.lines.clear();
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        let text = buffer(&state);
        assert!(text.contains("image"), "{text}");
        assert!(text.contains("4.0 KiB"), "{text}");
    }

    #[test]
    fn the_header_names_a_binary_file_with_its_kind() {
        let mut state = state();
        let doc = Doc {
            rel: PathBuf::from("logo.png"),
            name: "logo.png".into(),
            size: 4096,
            language: Language::Plain,
            lines: Vec::new(),
            truncated: false,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: Some(BinaryKind::Image),
            link_target: None,
            diff: None,
            diff_note: None,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        let text = buffer(&state);
        assert!(text.contains("logo.png"), "{text}");
        assert!(text.contains("image"), "{text}");
    }

    #[test]
    fn a_truncated_document_says_so_in_the_header() {
        let mut state = state();
        let doc = Doc {
            rel: PathBuf::from("huge.txt"),
            name: "huge.txt".into(),
            size: 9_000_000,
            language: Language::Text,
            lines: vec!["body".to_owned()],
            truncated: true,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        assert!(buffer(&state).contains("truncated"));
    }

    #[test]
    fn a_failed_load_says_it_cannot_read_the_file() {
        let mut state = state();
        state.preview.path = Some(PathBuf::from("locked.rs"));
        state.preview.preview = PreviewStatus::Failed;
        assert!(buffer(&state).contains("cannot read this file"));
    }

    #[test]
    fn a_load_in_flight_says_it_is_loading() {
        let mut state = state();
        state.preview.path = Some(PathBuf::from("a.rs"));
        state.preview.preview = PreviewStatus::Loading;
        assert!(buffer(&state).contains("loading"));
    }

    #[test]
    fn the_columns_the_pane_cannot_show_are_cut_off() {
        let mut state = state();
        state.preview.viewport = 4;
        let long = "x".repeat(200);
        let doc = Doc {
            rel: PathBuf::from("a.txt"),
            name: "a.txt".into(),
            size: 200,
            language: Language::Text,
            lines: vec![long],
            truncated: false,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        let text = buffer(&state);
        // The pane is 40 columns wide, so the line is cut with an ellipsis.
        assert!(text.contains('…'), "{text}");
        assert!(
            !text.contains(&"x".repeat(40)),
            "the line was not cut:\n{text}"
        );
    }

    /// A one-line document whose meanings the reader already knows.
    fn highlighted() -> AppState {
        let mut state = state();
        state.preview.viewport = 4;
        let doc = Doc {
            rel: PathBuf::from("a.rs"),
            name: "a.rs".into(),
            size: 19,
            language: Language::Rust,
            lines: vec!["let n = 42; // note".to_owned()],
            spans: vec![vec![
                highlight::Span {
                    start: 0,
                    end: 3,
                    semantic: Semantic::Type,
                },
                highlight::Span {
                    start: 6,
                    end: 7,
                    semantic: Semantic::Keyword,
                },
                highlight::Span {
                    start: 8,
                    end: 10,
                    semantic: Semantic::Number,
                },
                highlight::Span {
                    start: 12,
                    end: 19,
                    semantic: Semantic::Comment,
                },
            ]],
            truncated: false,
            too_large: false,
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        state
    }

    #[test]
    fn a_highlighted_line_is_drawn_in_its_meanings() {
        let buffer = draw(&highlighted());
        // The gutter takes five columns, so the line starts at column five.
        assert_eq!(buffer[(5, 1)].fg, Color::Yellow, "let");
        assert_eq!(buffer[(8, 1)].fg, Color::Reset, "the gap after it");
        assert_eq!(buffer[(11, 1)].fg, Color::Magenta, "=");
        assert_eq!(buffer[(13, 1)].fg, Color::Cyan, "42");
        assert_eq!(buffer[(17, 1)].fg, Color::DarkGray, "// note");
        assert!(buffer[(17, 1)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_terminal_without_colors_keeps_the_attributes() {
        let mut state = highlighted();
        state.theme = Theme::new(true);
        let buffer = draw(&state);
        for column in 5..=16 {
            assert_eq!(
                buffer[(column, 1)].fg,
                Color::Reset,
                "column {column} still had a color"
            );
        }
        let comment = &buffer[(17, 1)];
        assert_eq!(comment.fg, Color::Reset);
        assert!(
            comment.modifier.contains(Modifier::DIM),
            "a comment without color has nothing left to say"
        );
    }

    /// A state whose preview shows `baseline` and `current` as a diff.
    fn diffed(baseline: &str, current: &str, mode: PreviewMode) -> AppState {
        let mut state = state();
        state.preview.viewport = 6;
        state.preview_mode = mode;
        let diff = crate::core::diff::Diff::compute(baseline, current).expect("diff");
        let doc = Doc {
            rel: PathBuf::from("a.txt"),
            name: "a.txt".into(),
            size: current.len() as u64,
            language: Language::Text,
            lines: diff
                .rows
                .iter()
                .map(|row| match row {
                    Row::Context(line) | Row::Added(line) | Row::Removed(line) => line.clone(),
                    Row::Fold(count) => format!("\u{22ef} {count} lines"),
                })
                .collect(),
            truncated: false,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: Some(diff),
            diff_note: None,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        state
    }

    #[test]
    fn a_diff_row_is_drawn_with_its_marker_and_colour() {
        let state = diffed("one\ntwo\n", "one\nTWO\n", PreviewMode::InlineDiff);
        let buffer = draw(&state);
        // Row one is context: a blank marker and the pane's own text colour.
        assert_eq!(buffer[(4, 1)].fg, Color::Reset, "the context marker");
        // Row two is a removal, row three an addition: marker and text take
        // the colour of the side the row came from.
        assert_eq!(buffer[(4, 2)].fg, Color::Red, "the removal marker");
        assert_eq!(buffer[(5, 2)].symbol(), "t", "the removal text");
        assert_eq!(buffer[(5, 2)].fg, Color::Red, "the removal text");
        assert_eq!(buffer[(4, 3)].fg, Color::Green, "the addition marker");
        assert_eq!(buffer[(5, 3)].symbol(), "T", "the addition text");
        assert_eq!(buffer[(5, 3)].fg, Color::Green, "the addition text");
        // The number in front stays in the gutter's colour.
        assert_eq!(buffer[(3, 2)].symbol(), "2");
        assert_eq!(buffer[(3, 2)].fg, Color::DarkGray);
    }

    #[test]
    fn a_fold_is_drawn_as_a_marker_row() {
        let baseline = (1..=13)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let current = baseline.replace("line5", "changed");
        let mut state = diffed(&baseline, &current, PreviewMode::InlineDiff);
        // The fold sits in the trailing context, so scroll to it.
        state.preview.scroll_line = 6;
        let text = buffer(&state);
        assert!(text.contains("\u{22ef} 2 lines"), "{text}");
    }

    #[test]
    fn the_split_view_holds_the_two_sides_in_place() {
        let mut state = diffed(
            "keep\nold value\n",
            "keep\na longer new value\n",
            PreviewMode::SideBySide,
        );
        // The pane's own border is made of the same box drawing character, so
        // the divider is looked for where it stands: the middle of the body.
        let divider = |state: &AppState| draw(state)[(20, 1)].symbol().to_owned();

        // Below the split width the inline diff stands in for it.
        assert_ne!(divider(&state), "│", "the sides were split");
        let text = buffer(&state);
        assert!(text.contains("old value"), "{text}");

        // Wide enough, and the baseline moves to its own column.
        state.size.0 = 120;
        assert_eq!(divider(&state), "│", "no divider between the sides");
        let text = buffer(&state);
        assert!(text.contains("old value"), "{text}");
        assert!(text.contains("a longer"), "{text}");
    }

    #[test]
    fn the_split_columns_agree_with_the_side_rows_of_the_diff() {
        use crate::core::diff::{Diff, SideRow};

        let long = (1..=13)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let changed = long.replace("line5", "changed");
        for (baseline, current) in [
            ("x1\nx2\nkeep\n", "y1\ny2\ny3\nkeep\n"),
            ("x1\nx2\nx3\nkeep\n", "y1\nkeep\n"),
            (long.as_str(), changed.as_str()),
        ] {
            let diff = Diff::compute(baseline, current).expect("diff");
            // The fold texts live in the lines; nothing here compares them.
            let lines: Vec<String> = diff.rows.iter().map(|_| String::new()).collect();
            let mine: Vec<(Option<String>, Option<String>)> = side_cells(&diff.rows, &lines)
                .iter()
                .map(|[left, right]| (mark(left), mark(right)))
                .collect();
            let theirs: Vec<(Option<String>, Option<String>)> = diff
                .side_rows()
                .into_iter()
                .map(|row| match row {
                    SideRow::Context(text) => (Some(text.clone()), Some(text)),
                    SideRow::Replace { baseline, current } => (Some(baseline), Some(current)),
                    SideRow::Removed(text) => (Some(text), None),
                    SideRow::Added(text) => (None, Some(text)),
                    SideRow::Fold(_) => (Some("\u{22ef}".to_owned()), Some("\u{22ef}".to_owned())),
                })
                .collect();
            assert_eq!(mine, theirs, "for {baseline:?} against {current:?}");
        }

        fn mark(cell: &Cell<'_>) -> Option<String> {
            match cell {
                Cell::Shared(text) | Cell::Changed(text) => Some((*text).to_owned()),
                Cell::Fold(_) => Some("\u{22ef}".to_owned()),
                Cell::Blank => None,
            }
        }
    }
}
