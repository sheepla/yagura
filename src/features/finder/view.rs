//! Drawing a finder overlay: the query box, the list and the preview.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::state::{AppState, Overlay, PreviewMode};
use crate::core::text;
use crate::core::text::sanitize_os;
use crate::features::finder::grep;
use crate::features::preview;
use crate::ui::layout;
use crate::ui::theme::Role;
use crate::ui::{pane, placeholder};

/// Draws the overlay on top of the panes, which stay readable around it.
///
/// The popup's own area is cleared first: every cell no widget writes into
/// would otherwise keep the tree or the preview that was drawn behind it.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let areas = layout::finder(area);
    if areas.popup.width == 0 || areas.popup.height == 0 {
        return;
    }
    frame.render_widget(Clear, areas.popup);
    frame.render_widget(
        pane(
            state.overlay.label().unwrap_or("FILES"),
            &state.theme,
            false,
        ),
        areas.popup,
    );
    if areas.query.width == 0 || areas.query.height == 0 {
        return;
    }
    render_query(frame, areas.query, state);
    render_list(frame, areas.list, state);
    preview::view::render_header(frame, areas.header, state);
    // What sits beside the list is the file of the row under the cursor,
    // drawn whole: a diff of it would answer a question nobody asked (§13.3).
    preview::view::render_body_in(frame, areas.preview, state, PreviewMode::Content);
}

/// The query box, with a marker in front of what is typed (§13.1).
fn render_query(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width < 3 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "> ",
            state.theme.style(Role::PaneTitle),
        ))),
        Rect::new(area.x, area.y, 2, 1),
    );
    frame.render_widget(
        &state.finder.query,
        Rect::new(area.x + 2, area.y, area.width - 2, 1),
    );
}

/// The list: what matches now, or why there is nothing to show (§7).
fn render_list(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(pane(&title(state), &state.theme, false), area);
    let inner = area.inner(Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if matches!(state.overlay, Overlay::Grep)
        && state.finder.shown.is_empty()
        && let Some(error) = state.finder.grep.error.as_deref()
    {
        // What `rg` said of the pattern is the answer, not something to hide
        // behind another message (§7).
        placeholder(frame, inner, error, &state.theme);
        return;
    }
    if state.finder.shown.is_empty() {
        placeholder(frame, inner, hint(state), &state.theme);
        return;
    }
    // Only the rows that fit are turned into lines, so a hundred thousand
    // matches cost what the window can show of them.
    let start = state.finder.scroll.min(state.finder.shown.len());
    let lines: Vec<Line<'_>> = state.finder.shown[start..]
        .iter()
        .take(inner.height as usize)
        .enumerate()
        .map(|(offset, &index)| row_line(state, index, start + offset))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// What the list says about being empty (§7).
fn hint(state: &AppState) -> &'static str {
    if state.finder.scanning {
        return "scanning…";
    }
    if matches!(state.overlay, Overlay::Grep) {
        if state.finder.query_text().chars().count() < grep::MIN_PATTERN {
            return "type at least 2 characters";
        }
        if state.finder.grep.running {
            return "searching…";
        }
    }
    "no matches"
}

fn title(state: &AppState) -> String {
    let name = match state.overlay {
        Overlay::Grep => "grep",
        _ => "files",
    };
    let mut title = format!("{name} {}", state.finder.shown.len());
    if matches!(state.overlay, Overlay::Grep) && state.finder.grep.truncated {
        title.push_str(" (limit)");
    }
    if state.finder.scanning
        || (matches!(state.overlay, Overlay::Grep) && state.finder.grep.running)
    {
        title.push_str(" …");
    }
    title
}

/// One row: the selection is marked, and the row is shown as it stands (§13).
fn row_line<'a>(state: &'a AppState, index: u32, position: usize) -> Line<'a> {
    let selected = position == state.finder.selected;
    let marker = state.theme.style(Role::PaneBorder);
    let text = state.theme.style(if selected {
        Role::TreeSelected
    } else {
        Role::TreeText
    });
    let body = match state.overlay {
        // A hit reads as `path:line: text`, the way `rg` writes it, so a
        // row says where it is without a second column (§13.3).
        Overlay::Grep => {
            let hit = &state.finder.grep.matches[index as usize];
            format!(
                "{}:{}: {}",
                sanitize_os(hit.rel.as_os_str()),
                hit.line,
                text::sanitize(&hit.text)
            )
        }
        _ => sanitize_os(state.finder.found[index as usize].as_os_str()),
    };
    Line::from(vec![
        Span::styled(if selected { "> " } else { "  " }, marker),
        Span::styled(body, text),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::action::Action;
    use crate::app::command::Command;
    use crate::app::reduce::reduce;
    use crate::app::state::Config;
    use crate::cli::{BaselineKind, IconStyle};

    fn state(width: u16, height: u16) -> AppState {
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
            (width, height),
        )
    }

    fn text(state: &AppState) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
        terminal
            .draw(|frame| crate::app::view::render(frame, state))
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
    fn an_open_finder_shows_its_query_box_and_its_list() {
        let mut state = state(80, 24);
        reduce(&mut state, Action::Command(Command::OpenFiles));
        let drawn = text(&state);
        assert!(drawn.contains("FILES"), "{drawn}");
        assert!(
            drawn.contains("no matches") || drawn.contains("scanning"),
            "{drawn}"
        );
        // The status bar still says which mode the session is in (§10).
        assert!(drawn.contains("FILES"), "{drawn}");
    }

    #[test]
    fn an_open_finder_lists_what_the_walk_has_found() {
        let mut state = state(80, 24);
        reduce(&mut state, Action::Command(Command::OpenFiles));
        let generation = state.finder.scan_gen;
        reduce(
            &mut state,
            Action::FilesFound {
                generation,
                rels: vec![PathBuf::from("src/main.rs")],
            },
        );
        let drawn = text(&state);
        assert!(drawn.contains("src/main.rs"), "{drawn}");
        assert!(!drawn.contains("no matches"), "{drawn}");
    }

    #[test]
    fn a_finder_over_a_terminal_it_fits_draws_everything() {
        let mut state = state(60, 15);
        reduce(&mut state, Action::Command(Command::OpenFiles));
        let drawn = text(&state);
        assert!(drawn.contains("FILES"), "{drawn}");
        assert!(!drawn.contains("too small"), "{drawn}");
    }

    /// One hit, as a run reports it.
    fn hit(rel: &str, line: u32, text: &str) -> crate::features::finder::grep::GrepMatch {
        crate::features::finder::grep::GrepMatch {
            rel: PathBuf::from(rel),
            line,
            text: text.to_owned(),
            submatches: Vec::new(),
        }
    }

    /// The search overlay up, with `hits` waiting for the run of it.
    fn searching(
        state: &mut AppState,
        pattern: &str,
        hits: &[crate::features::finder::grep::GrepMatch],
    ) {
        reduce(state, Action::Command(Command::OpenGrep));
        for c in pattern.chars() {
            reduce(state, Action::Command(Command::FinderInsert(c)));
        }
        let generation = state.finder.grep.generation;
        reduce(
            state,
            Action::GrepChunk {
                generation,
                matches: hits.to_vec(),
            },
        );
    }

    #[test]
    fn the_search_says_what_it_wants_before_it_has_anything_to_show() {
        let mut state = state(80, 24);
        reduce(&mut state, Action::Command(Command::OpenGrep));
        let drawn = text(&state);
        assert!(drawn.contains("GREP"), "{drawn}");
        assert!(drawn.contains("type to search file contents"), "{drawn}");
        assert!(drawn.contains("type at least 2 characters"), "§7: {drawn}");
    }

    #[test]
    fn a_hit_reads_as_path_line_and_text() {
        let mut state = state(80, 24);
        searching(
            &mut state,
            "main",
            &[hit("src/main.rs", 7, "fn main() {"), hit("b.rs", 2, "x")],
        );
        let drawn = text(&state);
        assert!(drawn.contains("src/main.rs:7: fn main() {"), "{drawn}");
        assert!(drawn.contains("grep 2"), "{drawn}");
        assert!(!drawn.contains("no matches"), "{drawn}");
    }

    #[test]
    fn a_pattern_the_search_could_not_take_is_said_in_the_list() {
        let mut state = state(80, 24);
        reduce(&mut state, Action::Command(Command::OpenGrep));
        for c in "(a".chars() {
            reduce(&mut state, Action::Command(Command::FinderInsert(c)));
        }
        let generation = state.finder.grep.generation;
        reduce(
            &mut state,
            Action::GrepFailed {
                generation,
                message: "regex parse error: unclosed group".to_owned(),
            },
        );
        let drawn = text(&state);
        assert!(drawn.contains("regex parse error"), "{drawn}");
        assert!(!drawn.contains("no matches"), "{drawn}");
    }

    #[test]
    fn a_search_still_going_says_so_while_the_list_fills_up() {
        let mut state = state(80, 24);
        reduce(&mut state, Action::Command(Command::OpenGrep));
        for c in "ab".chars() {
            reduce(&mut state, Action::Command(Command::FinderInsert(c)));
        }
        let drawn = text(&state);
        assert!(drawn.contains("searching…"), "{drawn}");
        assert!(drawn.contains("grep 0 …"), "{drawn}");
    }
}
