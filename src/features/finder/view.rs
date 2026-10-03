//! Drawing a finder overlay: the query box, the list and the preview.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::AppState;
use crate::core::text::sanitize_os;
use crate::features::preview;
use crate::ui::layout;
use crate::ui::theme::Role;
use crate::ui::{pane, placeholder};

/// Draws the overlay on top of whatever the panes behind it are showing.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let areas = layout::finder(area);
    if areas.popup.width == 0 || areas.popup.height == 0 {
        return;
    }
    frame.render_widget(pane("FILES", &state.theme), areas.popup);
    if areas.query.width == 0 || areas.query.height == 0 {
        return;
    }
    render_query(frame, areas.query, state);
    render_list(frame, areas.list, state);
    preview::view::render_header(frame, areas.header, state);
    preview::view::render_body(frame, areas.preview, state);
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
    frame.render_widget(pane(&title(state), &state.theme), area);
    let inner = area.inner(Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if state.finder.shown.is_empty() {
        let hint = if state.finder.scanning {
            "scanning…"
        } else {
            "no matches"
        };
        placeholder(frame, inner, hint, &state.theme);
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

fn title(state: &AppState) -> String {
    let mut title = format!("files {}", state.finder.shown.len());
    if state.finder.scanning {
        title.push_str(" …");
    }
    title
}

/// One row: the selection is marked, and the path is shown as it is (§13).
fn row_line<'a>(state: &'a AppState, index: u32, position: usize) -> Line<'a> {
    let selected = position == state.finder.selected;
    let marker = state.theme.style(Role::PaneBorder);
    let text = state.theme.style(if selected {
        Role::TreeSelected
    } else {
        Role::TreeText
    });
    let rel = &state.finder.found[index as usize];
    Line::from(vec![
        Span::styled(if selected { "> " } else { "  " }, marker),
        Span::styled(sanitize_os(rel.as_os_str()), text),
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
}
