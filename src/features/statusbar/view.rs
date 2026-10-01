//! Drawing the status bar.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::{AppState, MessageKind};
use crate::ui::theme::Role;

use super::{mode_part, right_side};

/// Draws `[MODE] [message]` on the left and `[language] [N/M XXX%]` on the right.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let right = right_side(state);
    let right_width = crate::core::text::display_width(&right).min(area.width as usize);
    let left_width = (area.width as usize).saturating_sub(right_width);

    let mode_style = state.theme.style(Role::StatusMode);
    let message_style = match state
        .visible_message()
        .map(|message| message.kind)
        .unwrap_or(MessageKind::Info)
    {
        MessageKind::Info => state.theme.style(Role::StatusInfo),
        MessageKind::Error => state.theme.style(Role::StatusError),
    };

    // The mode is the part of the bar that always stays readable; the message
    // only gets the columns the mode did not need.
    let mode = mode_part(state);
    let mode_width = crate::core::text::display_width(&mode);
    let left_line = if mode_width <= left_width {
        Line::from(vec![
            Span::styled(mode, mode_style),
            Span::styled(
                crate::core::text::truncate_to_width(&message_part(state), left_width - mode_width),
                message_style,
            ),
        ])
    } else {
        Line::from(Span::styled(
            crate::core::text::truncate_to_width(&mode, left_width),
            mode_style,
        ))
    };
    frame.render_widget(Paragraph::new(left_line), area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            crate::core::text::truncate_to_width(&right, right_width),
            state.theme.style(Role::StatusPosition),
        ))),
        Rect::new(
            area.x + (area.width as usize).saturating_sub(right_width) as u16,
            area.y,
            right_width as u16,
            area.height,
        ),
    );
}

/// The message alone, so that it is styled separately from the mode.
fn message_part(state: &AppState) -> String {
    state
        .visible_message()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::app::state::{Config, Message, PreviewMode};
    use crate::cli::{BaselineKind, IconStyle};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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

    fn render_to_string(state: &AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), state))
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
    fn mode_language_and_position_are_all_visible() {
        let mut state = state();
        state.preview_mode = PreviewMode::InlineDiff;
        state.preview.language = "Rust".to_owned();
        let text = render_to_string(&state, 80, 1);
        assert!(text.contains("DIFF"), "{text}");
        assert!(text.contains("Rust"), "{text}");
        assert!(text.contains("1/1"), "{text}");
        assert!(text.contains("Top"), "{text}");
    }

    #[test]
    fn a_message_is_drawn_after_the_mode() {
        let mut state = state();
        state.set_message(Message::error("something failed"));
        let text = render_to_string(&state, 80, 1);
        assert!(text.contains("NORMAL"), "{text}");
        assert!(text.contains("something failed"), "{text}");
    }

    #[test]
    fn a_narrow_bar_keeps_the_mode_and_drops_the_message() {
        let mut state = state();
        state.set_message(Message::error("a very long error message indeed"));
        let text = render_to_string(&state, 26, 1);
        assert!(text.contains("NORMAL"), "{text}");
        assert!(!text.contains("very long"), "{text}");
        assert_eq!(crate::core::text::display_width(&text), 26, "{text:?}");
    }

    #[test]
    fn a_very_narrow_bar_never_overflows() {
        let mut state = state();
        state.set_message(Message::error("a very long error message indeed"));
        let text = render_to_string(&state, 20, 1);
        assert_eq!(crate::core::text::display_width(&text), 20, "{text:?}");
    }

    #[test]
    fn a_zero_sized_area_draws_nothing() {
        let state = state();
        let mut terminal = Terminal::new(TestBackend::new(10, 1)).expect("terminal");
        terminal
            .draw(|frame| render(frame, Rect::ZERO, &state))
            .expect("draw");
    }
}
