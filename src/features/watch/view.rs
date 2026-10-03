//! What the watch overlay draws.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::state::AppState;
use crate::core::fswatch::ChangeKind;
use crate::core::text;
use crate::ui::layout;
use crate::ui::theme::Role;
use crate::ui::{pane_line, placeholder};

/// Draws the log over the panes, which stay readable around it.
///
/// The popup's area is cleared first, so a row the log does not reach cannot
/// show the tree or the preview that was drawn behind it.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let popup = layout::watch(area);
    if popup.width == 0 || popup.height == 0 {
        return;
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(pane_line(title(state), &state.theme), popup);
    let inner = popup.inner(Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if state.watch.is_empty() {
        placeholder(frame, inner, "nothing has changed yet", &state.theme);
        return;
    }
    frame.render_widget(Paragraph::new(rows(state, inner.height as usize)), inner);
}

/// The title of the overlay, with what arrived while the reader was looking
/// away (§9).
fn title(state: &AppState) -> Line<'static> {
    let mut spans = vec![Span::styled("WATCH", state.theme.style(Role::PaneTitle))];
    if state.watch.unread() > 0 {
        spans.push(Span::styled(
            format!(" {} new", state.watch.unread()),
            state.theme.style(Role::StatusInfo),
        ));
    }
    Line::from(spans)
}

/// The rows the popup has room for, from the one the list is scrolled to.
fn rows(state: &AppState, viewport: usize) -> Vec<Line<'static>> {
    let first = state.watch.scroll();
    let last = (first + viewport).min(state.watch.len());
    (first..last)
        .filter_map(|index| row(state, index))
        .collect()
}

/// One line: when, what happened, where, and how often (§9).
fn row(state: &AppState, index: usize) -> Option<Line<'static>> {
    let entry = state.watch.get(index)?;
    let mut spans = vec![
        Span::styled(clock(entry.at), state.theme.style(Role::StatusMuted)),
        Span::raw(" "),
        Span::styled(
            entry.kind.label().to_owned(),
            state.theme.style(kind_role(entry.kind)),
        ),
        Span::raw(" "),
        Span::styled(
            text::sanitize_os(entry.rel.as_os_str()),
            state.theme.style(Role::TreeText),
        ),
    ];
    if entry.count > 1 {
        spans.push(Span::styled(
            format!(" \u{d7}{}", entry.count),
            state.theme.style(Role::StatusInfo),
        ));
    }
    if index == state.watch.selected_index() {
        // The selection is a background, so its modifiers are added to every
        // span of the row instead of replacing the span styles (§11).
        let highlight = state.theme.style(Role::TreeSelected).add_modifier;
        for span in &mut spans {
            span.style = span.style.add_modifier(highlight);
        }
    }
    Some(Line::from(spans))
}

/// The role a kind of change is drawn in, matching the marks in the tree.
fn kind_role(kind: ChangeKind) -> Role {
    match kind {
        ChangeKind::Create => Role::StatusAdded,
        ChangeKind::Modify => Role::StatusModified,
        ChangeKind::Remove => Role::StatusDeleted,
    }
}

/// The time of the change as the list shows it: `HH:MM:SS` (§9).
///
/// No time zone crate is in the design's list of dependencies, so this is
/// the change's own time in UTC rather than the reader's local one.
fn clock(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|passed| passed.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (seconds / 3600) % 24,
        (seconds / 60) % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::state::{Config, Overlay};
    use crate::cli::{BaselineKind, IconStyle};

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

    fn change(rel: &str, kind: ChangeKind) -> crate::core::fswatch::FsChange {
        crate::core::fswatch::FsChange {
            rel: PathBuf::from(rel),
            kind,
        }
    }

    /// Draws the overlay and hands back what landed on screen.
    fn drawn(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), state))
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
    fn an_empty_log_says_so_rather_than_showing_a_hole() {
        let mut state = state(80, 24);
        state.overlay = Overlay::Watch;
        let drawn = drawn(&state, 80, 24);
        assert!(drawn.contains("WATCH"), "{drawn}");
        assert!(drawn.contains("nothing has changed yet"), "{drawn}");
    }

    #[test]
    fn a_row_says_when_what_and_where() {
        let mut state = state(80, 24);
        state.overlay = Overlay::Watch;
        state.watch.record(
            &[change("src/main.rs", ChangeKind::Create)],
            UNIX_EPOCH + std::time::Duration::from_secs(3661),
        );
        state.watch.open();
        let drawn = drawn(&state, 80, 24);
        assert!(drawn.contains("01:01:01"), "{drawn}");
        assert!(drawn.contains("create"), "{drawn}");
        assert!(drawn.contains("src/main.rs"), "{drawn}");
        assert!(
            !drawn.contains("\u{d7}"),
            "one change is not a count of them"
        );
    }

    #[test]
    fn changes_that_merged_are_shown_as_a_count() {
        let mut state = state(80, 24);
        state.overlay = Overlay::Watch;
        let at = UNIX_EPOCH + std::time::Duration::from_secs(10);
        state
            .watch
            .record(&[change("a.txt", ChangeKind::Modify)], at);
        state
            .watch
            .record(&[change("a.txt", ChangeKind::Modify)], at);
        state.watch.open();
        let drawn = drawn(&state, 80, 24);
        assert!(drawn.contains("\u{d7}2"), "{drawn}");
    }

    #[test]
    fn what_arrived_while_the_reader_was_elsewhere_is_in_the_title() {
        let mut state = state(80, 24);
        state.overlay = Overlay::Watch;
        state
            .watch
            .record(&[change("a.txt", ChangeKind::Modify)], UNIX_EPOCH);
        state.watch.record(
            &[change("b.txt", ChangeKind::Create)],
            UNIX_EPOCH + std::time::Duration::from_secs(5),
        );
        state.watch.open();
        state.watch.select_first();
        let at = UNIX_EPOCH + std::time::Duration::from_secs(10);
        state
            .watch
            .record(&[change("c.txt", ChangeKind::Remove)], at);
        assert_eq!(state.watch.unread(), 1);
        let drawn = drawn(&state, 80, 24);
        assert!(drawn.contains("1 new"), "{drawn}");
    }

    #[test]
    fn the_time_is_the_hour_minute_and_second_of_the_change() {
        assert_eq!(clock(UNIX_EPOCH), "00:00:00");
        assert_eq!(
            clock(UNIX_EPOCH + std::time::Duration::from_secs(86399)),
            "23:59:59"
        );
        assert_eq!(
            clock(UNIX_EPOCH - std::time::Duration::from_secs(5)),
            "00:00:00",
            "a clock that has run backwards shows no time rather than a wrong one"
        );
    }

    #[test]
    fn only_the_rows_that_fit_are_drawn() {
        let mut state = state(60, 12);
        state.overlay = Overlay::Watch;
        for index in 0..100 {
            state.watch.record(
                &[change(&format!("f{index}.txt"), ChangeKind::Modify)],
                UNIX_EPOCH + std::time::Duration::from_secs(index as u64),
            );
        }
        state.watch.open();
        state
            .watch
            .scroll_into_view(layout::watch_rows(Rect::new(0, 0, 60, 12)) as usize);
        let lines = rows(&state, layout::watch_rows(Rect::new(0, 0, 60, 12)) as usize);
        assert_eq!(
            lines.len(),
            layout::watch_rows(Rect::new(0, 0, 60, 12)) as usize,
            "one line per row the popup has room for"
        );
    }

    #[test]
    fn a_row_is_muted_and_the_selection_stands_out() {
        let mut state = state(80, 24);
        state.overlay = Overlay::Watch;
        state
            .watch
            .record(&[change("a.txt", ChangeKind::Modify)], UNIX_EPOCH);
        state.watch.record(
            &[change("b.txt", ChangeKind::Remove)],
            UNIX_EPOCH + std::time::Duration::from_secs(10),
        );
        state.watch.open();
        let selected = row(&state, 1).expect("the row");
        assert!(selected.spans.iter().all(|span| {
            span.style
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED)
        }));
        let above = row(&state, 0).expect("the row");
        assert!(!above.spans.iter().any(|span| {
            span.style
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED)
        }));
    }
}
