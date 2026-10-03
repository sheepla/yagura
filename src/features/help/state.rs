//! How far down the help list the reader has scrolled.

/// The list of keys is longer than any terminal it is drawn on, so the
/// overlay keeps where the reader is (§5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HelpState {
    pub scroll: usize,
}

impl HelpState {
    /// Back to the top: an overlay opens where the list begins.
    pub fn open(&mut self) {
        self.scroll = 0;
    }

    /// Moves by `by` lines. Past the end there is nothing to read, and above
    /// the first line there is nowhere to go.
    pub fn scroll_by(&mut self, by: isize, last: usize) {
        let at = self.scroll as isize;
        self.scroll = at.saturating_add(by).clamp(0, last as isize) as usize;
    }

    /// Goes to a line of the list, the far end included.
    pub fn scroll_to(&mut self, line: usize, last: usize) {
        self.scroll = line.min(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_starts_at_the_top_when_it_is_opened() {
        let mut help = HelpState { scroll: 40 };
        help.open();
        assert_eq!(help.scroll, 0);
    }

    #[test]
    fn the_reader_cannot_scroll_past_either_end() {
        let mut help = HelpState::default();
        help.scroll_by(-1, 10);
        assert_eq!(help.scroll, 0, "nothing above the first line");
        help.scroll_to(10, 10);
        assert_eq!(help.scroll, 10);
        help.scroll_by(1, 10);
        assert_eq!(help.scroll, 10, "nothing below the last line");
        help.scroll_by(-3, 10);
        assert_eq!(help.scroll, 7);
    }

    #[test]
    fn a_line_past_the_end_is_the_end() {
        let mut help = HelpState::default();
        help.scroll_to(99, 5);
        assert_eq!(help.scroll, 5);
    }
}
