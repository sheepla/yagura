//! Rendering helpers shared by the panes.

pub mod elapsed;
pub mod layout;
pub mod theme;

use std::io::{self, Write};

use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use theme::{Role, Theme};

/// DEC 2026 "synchronized output": everything between the two sequences lands
/// on screen in one go. Terminals that do not know it ignore the sequence.
const SYNC_BEGIN: &[u8] = b"\x1b[?2026h";
const SYNC_END: &[u8] = b"\x1b[?2026l";

/// Wraps a frame in synchronized output markers.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameSync {
    enabled: bool,
}

impl FrameSync {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Starts a synchronized update.
    pub fn begin(&self) -> io::Result<()> {
        self.write(SYNC_BEGIN)
    }

    /// Ends a synchronized update.
    pub fn end(&self) -> io::Result<()> {
        self.write(SYNC_END)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let mut stdout = io::stdout();
        stdout.write_all(bytes)?;
        stdout.flush()
    }
}

/// A bordered pane with a title.
pub fn pane<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    pane_line(
        Line::from(Span::styled(title, theme.style(Role::PaneTitle))),
        theme,
    )
}

/// A bordered pane with a title built from separately styled pieces.
pub fn pane_line<'a>(title: Line<'a>, theme: &Theme) -> Block<'a> {
    Block::bordered()
        .title(title)
        .border_style(theme.style(Role::PaneBorder))
}

/// Draws a dim, centered hint where content would go.
pub fn placeholder(frame: &mut Frame<'_>, area: ratatui::layout::Rect, text: &str, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_owned(),
            theme.style(Role::PreviewPlaceholder),
        )))
        .centered(),
        area,
    );
}

/// The whole screen becomes a single hint when the terminal is too small.
pub fn render_too_small(frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
    let text = format!(
        "terminal too small ({}x{}, minimum {}x{})",
        area.width,
        area.height,
        layout::MIN_WIDTH,
        layout::MIN_HEIGHT
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, theme.style(Role::TooSmall)))).centered(),
        area,
    );
}

/// Style for text that carries a status, such as an error marker.
pub fn muted(theme: &Theme) -> Style {
    theme.style(Role::StatusMuted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_output_is_off_when_disabled() {
        let sync = FrameSync::new(false);
        assert!(!sync.is_enabled());
        sync.begin().expect("no write");
        sync.end().expect("no write");
    }

    #[test]
    fn sync_output_markers_are_a_pair() {
        assert_eq!(SYNC_BEGIN, b"\x1b[?2026h");
        assert_eq!(SYNC_END, b"\x1b[?2026l");
    }
}
