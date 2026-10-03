//! What the help overlay draws.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::state::AppState;
use crate::input::keymap::{self, HelpSection};
use crate::ui::layout;
use crate::ui::theme::Role;
use crate::ui::{muted, pane};

/// A note no key of the table can say, drawn under the finder's rows.
const FINDER_NOTE: &str = "Plain characters go to the query box.";

/// Draws the list over the panes, which stay readable around it.
///
/// The popup's area is cleared first, so a row the table does not reach
/// cannot show the tree or the preview that was drawn behind it.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let popup = layout::help(area);
    if popup.width == 0 || popup.height == 0 {
        return;
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(pane("HELP", &state.theme), popup);
    let inner = popup.inner(Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines = lines(state);
    // The reducer clamps the same way, so a resize can leave the state above
    // the end without the pane going blank.
    let last = lines.len().saturating_sub(inner.height as usize);
    let scroll = state.help.scroll.min(last) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

/// How many lines the whole overlay draws, headings and notes included.
pub fn content_height(state: &AppState) -> usize {
    lines(state).len()
}

/// The list itself: a heading per context, then the rows of its table.
fn lines(state: &AppState) -> Vec<Line<'static>> {
    let sections = keymap::help_sections();
    let width = key_width(&sections);
    let mut lines = Vec::new();
    for section in &sections {
        lines.push(Line::from(Span::styled(
            section.title.to_owned(),
            state.theme.style(Role::PaneTitle),
        )));
        for row in &section.rows {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<width$}", row.key, width = width),
                    state.theme.style(Role::StatusInfo),
                ),
                Span::raw("  "),
                Span::styled(row.does.to_owned(), state.theme.style(Role::PreviewText)),
            ]));
        }
        if section.title == "Finder" {
            lines.push(Line::from(Span::styled(
                FINDER_NOTE.to_owned(),
                muted(&state.theme),
            )));
        }
        lines.push(Line::default());
    }
    lines
}

/// How wide the column of keys is, so that every description lines up.
fn key_width(sections: &[HelpSection]) -> usize {
    sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .map(|row| row.key.len())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::state::{AppState, Config, Overlay};
    use crate::cli::{BaselineKind, IconStyle};
    use std::path::PathBuf;

    fn state(width: u16, height: u16) -> AppState {
        let config = Config {
            root: PathBuf::from("/root"),
            icons: IconStyle::Text,
            baseline: BaselineKind::Auto,
            ignore_enabled: true,
            no_color: false,
            log: None,
            selftest_panic: false,
        };
        AppState::new(config, (width, height))
    }

    /// Draws the overlay over the panes and hands back what landed on screen.
    fn drawn(width: u16, height: u16) -> String {
        let state = state(width, height);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), &state))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn the_overlay_shows_the_sections_and_the_keys_under_them() {
        // Tall enough that the pane holds the whole table.
        let drawn = drawn(100, 120);
        assert!(drawn.contains("Everywhere"), "{drawn}");
        assert!(drawn.contains("Tree"), "{drawn}");
        assert!(drawn.contains("Preview"), "{drawn}");
        assert!(drawn.contains("Finder"), "{drawn}");
        assert!(drawn.contains("Q"), "{drawn}");
        assert!(drawn.contains("Quit"), "{drawn}");
    }

    #[test]
    fn every_row_of_the_table_is_drawn() {
        let state = state(100, 60);
        let lines = lines(&state);
        let drawn: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        let rows = keymap::help_sections()
            .iter()
            .flat_map(|section| section.rows.iter().map(|row| row.key.clone()))
            .collect::<Vec<_>>();
        for key in rows {
            assert!(
                drawn.iter().any(|line| line.starts_with(&key)),
                "the row for {key} was not drawn"
            );
        }
    }

    #[test]
    fn a_list_longer_than_the_pane_is_scrolled_rather_than_shrunk() {
        let mut state = state(80, 15);
        state.overlay = Overlay::Help;
        assert!(
            content_height(&state) > layout::help_rows(Rect::new(0, 0, 80, 15)) as usize,
            "the list is meant to be longer than a small terminal"
        );
        state.help.scroll = content_height(&state);
        let drawn = {
            let backend = TestBackend::new(80, 15);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| render(frame, frame.area(), &state))
                .expect("draw");
            let buffer = terminal.backend().buffer();
            let mut text = String::new();
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    text.push_str(buffer[(x, y)].symbol());
                }
                text.push('\n');
            }
            text
        };
        assert!(drawn.contains("HELP"), "the frame of the overlay");
    }
}
