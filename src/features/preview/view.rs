//! The preview header and body.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::state::AppState;
use crate::core::highlight::{self, Semantic};
use crate::core::preview::{Doc, human_size};
use crate::core::text::sanitize_os;
use crate::ui::theme::{Role, Theme};
use crate::ui::{pane, placeholder};

use super::state::PreviewStatus;

/// Columns the line numbers take in front of every line.
pub const GUTTER_COLS: usize = 5;

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
    frame.render_widget(pane(&title, &state.theme), area);
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
/// Only the rows the pane can show are built, so a 200,000 line document costs
/// the same as a 20 line one.
fn body<'a>(state: &'a AppState, doc: &'a Doc, area: Rect) -> Text<'a> {
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
}
