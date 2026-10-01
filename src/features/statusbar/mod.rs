//! The status bar.

pub mod view;

use crate::app::state::AppState;

/// Where a scroll position sits, rendered Vim style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollPos {
    Top,
    Bottom,
    Percent(u16),
}

impl ScrollPos {
    pub fn label(self) -> String {
        match self {
            Self::Top => "Top".to_owned(),
            Self::Bottom => "Bot".to_owned(),
            Self::Percent(value) => format!("{value}%"),
        }
    }
}

/// Vim style percentage for a scroll position.
///
/// `first` is the index of the topmost visible row and `height` the number of
/// rows on screen.
pub fn scroll_position(first: usize, total: usize, height: usize) -> ScrollPos {
    if total == 0 || height == 0 {
        return ScrollPos::Top;
    }
    if first == 0 {
        return ScrollPos::Top;
    }
    let visible = height.min(total);
    if first + visible >= total {
        return ScrollPos::Bottom;
    }
    let span = total - visible;
    if span == 0 {
        return ScrollPos::Bottom;
    }
    let percent = (first as u32 * 100 / span as u32).min(100) as u16;
    if percent >= 100 {
        ScrollPos::Bottom
    } else {
        ScrollPos::Percent(percent)
    }
}

/// The mode alone, which is the part of the bar that is never dropped.
pub fn mode_part(state: &AppState) -> String {
    let mut left = String::new();
    left.push(' ');
    left.push_str(
        state
            .overlay
            .label()
            .unwrap_or_else(|| state.preview_mode.label()),
    );
    if state.follow {
        left.push_str(" [F]");
    }
    left.push(' ');
    left
}

/// The left hand side: mode and message.
pub fn left_side(state: &AppState) -> String {
    let mut left = mode_part(state);
    if let Some(message) = state.visible_message() {
        left.push_str(&crate::core::text::sanitize(&message.text));
    }
    left
}

/// The right hand side: language and position.
pub fn right_side(state: &AppState) -> String {
    let (position, total) = match state.focus {
        crate::app::state::Focus::Tree => (
            state.tree.selected_index().map_or(0, |index| index + 1),
            state.tree.row_count(),
        ),
        crate::app::state::Focus::Preview => (state.preview.position(), state.preview.total_lines),
    };
    let height = match state.focus {
        crate::app::state::Focus::Tree => state.tree.viewport,
        crate::app::state::Focus::Preview => state.preview_viewport_height(),
    };
    let position_label = scroll_position(first_index(state, total), total, height).label();
    format!(
        " {} {}/{} {} ",
        state.preview.language_label(),
        position,
        total,
        position_label
    )
}

/// The first row shown, which is the scroll offset for the focused pane.
fn first_index(state: &AppState, total: usize) -> usize {
    let (first, _height) = match state.focus {
        crate::app::state::Focus::Tree => (state.tree.scroll, state.tree.viewport),
        crate::app::state::Focus::Preview => {
            (state.preview.scroll_line, state.preview_viewport_height())
        }
    };
    first.min(total.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::app::state::{Config, Focus, MessageKind, Overlay, PreviewMode};
    use crate::cli::{BaselineKind, IconStyle};

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

    #[test]
    fn the_percentage_is_vim_style() {
        assert_eq!(scroll_position(0, 340, 22), ScrollPos::Top);
        assert_eq!(scroll_position(339, 340, 22), ScrollPos::Bottom);
        assert_eq!(scroll_position(318, 340, 22), ScrollPos::Bottom);
        assert_eq!(scroll_position(0, 0, 22), ScrollPos::Top);
        assert_eq!(scroll_position(10, 0, 0), ScrollPos::Top);
        assert_eq!(scroll_position(50, 340, 22), ScrollPos::Percent(15));
    }

    #[test]
    fn percentages_stay_inside_the_range() {
        for first in 0..400usize {
            let label = scroll_position(first, 400, 20).label();
            let percent = label.trim_end_matches('%');
            assert!(
                matches!(label.as_str(), "Top" | "Bot")
                    || percent.parse::<u16>().is_ok_and(|value| value < 100),
                "{label} is not a Vim percentage"
            );
        }
    }

    #[test]
    fn the_mode_comes_from_the_overlay_or_the_preview_mode() {
        let mut state = state();
        assert!(left_side(&state).contains("NORMAL"));
        state.preview_mode = PreviewMode::SideBySide;
        assert!(left_side(&state).contains("SPLIT"));
        state.overlay = Overlay::Grep;
        assert!(left_side(&state).contains("GREP"));
        assert!(!left_side(&state).contains("SPLIT"));
    }

    #[test]
    fn follow_mode_is_marked_after_the_mode() {
        let mut state = state();
        state.follow = true;
        assert!(left_side(&state).starts_with(" NORMAL [F]"));
    }

    #[test]
    fn messages_are_sanitized_before_they_are_shown() {
        let mut state = state();
        state.set_error("\u{1b}[2Jboom");
        assert!(left_side(&state).contains("\u{241b}[2Jboom"));
    }

    #[test]
    fn the_right_side_reports_the_tree_position_with_the_tree_focused() {
        let state = state();
        let right = right_side(&state);
        assert_eq!(right, " plain 1/1 Top ");
    }

    #[test]
    fn the_right_side_reports_the_preview_position_with_the_preview_focused() {
        let mut state = state();
        state.focus = Focus::Preview;
        state.preview.total_lines = 340;
        state.preview.scroll_line = 100;
        let right = right_side(&state);
        assert!(right.starts_with(" plain 101/340 "));
        assert!(right.ends_with("% "));
    }

    #[test]
    fn an_error_message_outlives_an_info_message() {
        let mut state = state();
        state.set_message(crate::app::state::Message::info("note", state.now));
        assert_eq!(
            state.visible_message().map(|m| m.kind),
            Some(MessageKind::Info)
        );
        state.set_message(crate::app::state::Message::error("bad"));
        assert_eq!(
            state.visible_message().map(|m| m.kind),
            Some(MessageKind::Error)
        );
    }

    #[test]
    fn the_root_name_is_never_shown_as_an_empty_header() {
        let state = state();
        assert_eq!(state.root(), PathBuf::from("/root"));
    }
}
