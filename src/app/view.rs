//! Drawing a whole frame.

use ratatui::Frame;

use crate::app::state::AppState;
use crate::features::{preview, statusbar, tree};
use crate::ui::layout;

/// Draws one frame. Reads the state only, never writes it (invariant I7).
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    if layout::too_small(area) {
        crate::ui::render_too_small(frame, area, &state.theme);
        return;
    }
    let areas = layout::compute(area, state.tree_visible);
    if state.tree_visible {
        tree::view::render(frame, areas.tree, state);
    }
    preview::view::render_header(frame, areas.header, state);
    preview::view::render_body(frame, areas.preview, state);
    statusbar::view::render(frame, areas.status, state);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::state::Config;
    use crate::cli::{BaselineKind, IconStyle};

    fn state(width: u16, height: u16, no_color: bool) -> AppState {
        AppState::new(
            Config {
                root: PathBuf::from("/root"),
                icons: IconStyle::Text,
                baseline: BaselineKind::Auto,
                ignore_enabled: true,
                no_color,
                log: None,
                selftest_panic: false,
            },
            (width, height),
        )
    }

    fn text(state: &AppState) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
        terminal.draw(|frame| render(frame, state)).expect("draw");
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
    fn a_small_terminal_only_says_so() {
        let text = text(&state(40, 10, false));
        assert!(text.contains("terminal too small"), "{text}");
        assert!(!text.contains("NORMAL"), "{text}");
    }

    #[test]
    fn the_minimum_size_is_enough_for_the_whole_layout() {
        let text = text(&state(60, 15, false));
        assert!(!text.contains("too small"), "{text}");
        assert!(text.contains("NORMAL"), "{text}");
    }

    #[test]
    fn hiding_the_tree_gives_the_width_to_the_preview() {
        let mut state = state(80, 24, false);
        let with_tree = text(&state);
        state.tree_visible = false;
        let without_tree = text(&state);
        assert!(with_tree.contains("│"), "{with_tree}");
        assert!(!without_tree.contains('│'), "{without_tree}");
    }
}
