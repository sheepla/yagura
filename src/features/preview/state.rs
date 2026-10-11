//! Preview state.
//!
//! The document itself is built by [`crate::core::preview`] on a blocking
//! worker; what lives here is the shell around it: which file is shown, which
//! generation of content is current, where it is scrolled to.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::state::PreviewMode;
use crate::core::preview::{Doc, Language};
use crate::features::finder::grep::{GrepMatch, display_ranges};

/// Where a grep hit sits in the file the pane is showing (§13.3).
///
/// It is a mark on a body rather than part of it: the pane draws it while it
/// is pointed at the file it belongs to, and shows nothing of it once it is
/// not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MatchFocus {
    /// File the hit is in.
    pub rel: PathBuf,
    /// Line the hit is on, one based.
    pub line: u32,
    /// Where on that line the hit lands, in bytes of the drawn text.
    pub ranges: Vec<(usize, usize)>,
}

/// What the preview pane is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PreviewStatus {
    /// Nothing is selected, or a directory is.
    #[default]
    Empty,
    /// A load is in flight and no document has arrived yet.
    Loading,
    /// A document is on screen.
    Ready,
    /// The load failed; the message is on the status bar.
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct PreviewState {
    /// File the preview points at, `None` while a directory is selected.
    pub path: Option<PathBuf>,
    /// Incremented on every new target so that late results can be dropped.
    pub generation: u64,
    /// The mode the current read was asked for, so that a reader whose mode
    /// the finder traded away knows the file has to be read again (§13.2).
    pub mode: PreviewMode,
    /// What the pane is showing.
    pub preview: PreviewStatus,
    /// The document, shared with the renderer and with a reload in flight.
    pub doc: Option<Arc<Doc>>,
    /// First visible line, zero based.
    pub scroll_line: usize,
    /// Leftmost visible column.
    pub scroll_col: usize,
    /// Lines in the document.
    pub total_lines: usize,
    /// Name of the detected language, `plain` when nothing matched.
    pub language: String,
    /// Lines the pane can show; set from the layout when the size changes.
    pub viewport: usize,
    /// The hit the pane is aimed at, while one is set.
    pub focus: Option<MatchFocus>,
    /// Line to bring to the middle once the body of the aimed file is there.
    centre_on: Option<u32>,
}

impl PreviewState {
    /// Points the preview at a new file and starts a new generation.
    ///
    /// The document of the previous file is dropped here, because a scroll
    /// position and a body from two different files do not belong together.
    pub fn target(&mut self, rel: PathBuf) -> u64 {
        self.generation += 1;
        self.path = Some(rel);
        self.preview = PreviewStatus::Loading;
        self.doc = None;
        self.scroll_line = 0;
        self.scroll_col = 0;
        self.total_lines = 0;
        self.language = Language::Plain.label().to_owned();
        self.generation
    }

    /// Forgets the target, e.g. when a directory is selected.
    pub fn clear(&mut self) {
        self.path = None;
        self.preview = PreviewStatus::Empty;
        self.doc = None;
        self.scroll_line = 0;
        self.scroll_col = 0;
        self.total_lines = 0;
        self.language = Language::Plain.label().to_owned();
        self.drop_focus();
    }

    /// The language label for the status bar.
    pub fn language_label(&self) -> &str {
        if self.language.is_empty() {
            "plain"
        } else {
            &self.language
        }
    }

    /// Line shown first, one based, as the status bar reports it.
    pub fn position(&self) -> usize {
        if self.total_lines == 0 {
            0
        } else {
            self.scroll_line + 1
        }
    }

    /// The lines the pane can show right now.
    pub fn height(&self) -> usize {
        self.viewport.max(1)
    }

    /// The last line the pane can scroll to.
    pub fn max_scroll(&self) -> usize {
        self.total_lines.saturating_sub(self.height())
    }

    /// How far the view is from the end, for the `Top`/`Bot`/`NN%` label.
    pub fn at_bottom(&self) -> bool {
        self.total_lines == 0 || self.scroll_line >= self.max_scroll()
    }

    /// Keeps the scroll inside the document.
    ///
    /// A reload can make the document shorter, and a resize can make the pane
    /// shorter, so every change to the geometry ends here. The column is
    /// capped by the widest line the document has, so a rewrite that took the
    /// width away cannot leave the view starting past every line it holds.
    pub fn clamp(&mut self) {
        self.scroll_line = self.scroll_line.min(self.max_scroll());
        self.scroll_col = self.scroll_col.min(self.widest_line().saturating_sub(1));
    }

    /// Scrolls by whole lines, never past either end.
    pub fn scroll_by(&mut self, delta: isize) {
        let target = self.scroll_line as isize + delta;
        self.scroll_line = target.clamp(0, self.max_scroll() as isize) as usize;
    }

    /// The widest line of the document, in columns.
    pub fn widest_line(&self) -> usize {
        self.doc.as_ref().map_or(0, |doc| {
            doc.lines
                .iter()
                .map(|line| crate::core::text::display_width(line))
                .max()
                .unwrap_or(0)
        })
    }

    /// The rightmost column the view can start at, so that the widest line
    /// still fills the pane instead of scrolling into empty space.
    pub fn max_scroll_col(&self, width: usize) -> usize {
        self.widest_line().saturating_sub(width)
    }

    /// Scrolls horizontally, never past the widest line.
    pub fn scroll_by_columns(&mut self, delta: isize, width: usize) {
        let target = self.scroll_col as isize + delta;
        self.scroll_col = target.clamp(0, self.max_scroll_col(width) as isize) as usize;
    }

    /// True when a result belongs to the target that is on screen.
    ///
    /// The generation is the only thing that says so (invariant I3): a mismatch
    /// means the user has moved on and the result is stale.
    pub fn is_current(&self, rel: &PathBuf, generation: u64) -> bool {
        generation == self.generation && self.path.as_ref() == Some(rel)
    }

    /// Takes the document that just arrived.
    pub fn set_doc(&mut self, doc: Doc) {
        self.total_lines = doc.lines.len();
        self.language = doc.status_label().to_owned();
        self.doc = Some(Arc::new(doc));
        self.preview = PreviewStatus::Ready;
        // A shorter document must not leave the view below its end.
        self.clamp();
        self.centre_here();
    }

    /// Takes the colours that finish the load, keeping the body that is on
    /// screen (design §9.3). Nothing but the meanings changes, so the view
    /// does not move.
    pub fn set_highlights(
        &mut self,
        spans: Vec<Vec<crate::core::highlight::Span>>,
        truncated: bool,
    ) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let mut doc = (**doc).clone();
        doc.spans = spans;
        doc.highlight_truncated = truncated;
        self.doc = Some(Arc::new(doc));
    }

    /// Takes the body away while the read for another kind of view is on
    /// its way (spec §6).
    ///
    /// A diff folds its rows into `lines`, so a body built for one kind of
    /// view would be drawn wrongly by the other. The file and the scroll
    /// stay, so the view is where it was once the right body arrives.
    pub fn drop_body(&mut self) {
        self.doc = None;
        self.preview = PreviewStatus::Loading;
    }

    /// Records that the load failed; the message is on the status bar.
    pub fn set_failed(&mut self) {
        self.doc = None;
        self.total_lines = 0;
        self.preview = PreviewStatus::Failed;
        self.clamp();
    }

    /// Aims the pane at one hit, or takes the aim away when there is none
    /// (§13.3).
    ///
    /// The same hit again means nothing has moved, so a scroll of the
    /// reader's own is left where it is.
    pub fn aim_at(&mut self, hit: Option<&GrepMatch>) {
        let focus = hit.map(|hit| MatchFocus {
            rel: hit.rel.clone(),
            line: hit.line,
            ranges: display_ranges(&hit.text, &hit.submatches),
        });
        if self.focus == focus {
            return;
        }
        self.focus = focus;
        self.centre_on = self.focus.as_ref().map(|focus| focus.line);
        self.centre_here();
    }

    /// The pane has been pointed at `rel`: a mark on some other file means
    /// nothing there, so it goes (§13.3).
    pub fn aimed_at(&mut self, rel: &Path) {
        if self.focus.as_ref().is_some_and(|focus| focus.rel != rel) {
            self.drop_focus();
        }
    }

    /// The mark on the shown line, if the pane is showing the file it is on.
    ///
    /// A mark is only ever read through here, which is what keeps it from
    /// lighting up rows of some other file.
    pub fn marks(&self) -> Option<&MatchFocus> {
        self.focus
            .as_ref()
            .filter(|focus| self.path.as_ref() == Some(&focus.rel))
    }

    /// Brings the aimed line to the middle of the pane, once its body is
    /// there to scroll (§4).
    fn centre_here(&mut self) {
        let Some(line) = self.centre_on else {
            return;
        };
        if self.focus.is_none() {
            // Nothing to centre on any more.
            self.centre_on = None;
            return;
        }
        if self.path.as_ref() != self.focus.as_ref().map(|focus| &focus.rel) {
            // The mark is for a file the pane is not showing; the aim may
            // still move, so the line waits where it is.
            return;
        }
        if self.total_lines == 0 {
            return;
        }
        self.centre_on = None;
        self.scroll_line = (line as usize).saturating_sub(1);
        self.scroll_line = self.scroll_line.saturating_sub(self.height() / 2);
        self.clamp();
    }

    /// Forgets the mark and anything still owed to it.
    fn drop_focus(&mut self) {
        self.focus = None;
        self.centre_on = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::preview::PreviewError;

    /// Delivers a result the way the reducer does.
    fn accept(
        preview: &mut PreviewState,
        rel: &str,
        generation: u64,
        result: Result<Doc, PreviewError>,
    ) -> bool {
        if !preview.is_current(&PathBuf::from(rel), generation) {
            return false;
        }
        match result {
            Ok(doc) => preview.set_doc(doc),
            Err(_) => preview.set_failed(),
        }
        true
    }

    fn doc(rel: &str, lines: &[&str]) -> Doc {
        Doc {
            rel: PathBuf::from(rel),
            name: rel.into(),
            size: 1,
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
            formatted: false,
        }
    }

    #[test]
    fn an_empty_preview_reports_plain_and_no_position() {
        let preview = PreviewState::default();
        assert_eq!(preview.language_label(), "plain");
        assert_eq!(preview.position(), 0);
        assert_eq!(preview.generation, 0);
    }

    #[test]
    fn a_shorter_document_pulls_the_column_back_into_it() {
        let mut preview = PreviewState::default();
        preview.set_doc(doc("a.rs", &["let wide = 0000000000;"]));
        preview.scroll_col = 5;
        // The file is rewritten without anything like its old width.
        preview.set_doc(doc("a.rs", &["x"]));
        assert_eq!(
            preview.scroll_col, 0,
            "the view cannot start past every line the document holds"
        );
    }

    #[test]
    fn the_position_is_one_based_and_starts_at_one() {
        let mut preview = PreviewState {
            total_lines: 340,
            viewport: 10,
            ..Default::default()
        };
        assert_eq!(preview.position(), 1);
        preview.scroll_line = 11;
        assert_eq!(preview.position(), 12);
    }

    #[test]
    fn the_language_label_falls_back_to_plain() {
        let preview = PreviewState {
            language: "rust".to_owned(),
            ..Default::default()
        };
        assert_eq!(preview.language_label(), "rust");
    }

    #[test]
    fn targeting_a_file_advances_the_generation_and_clears_the_body() {
        let mut preview = PreviewState::default();
        preview.target(PathBuf::from("a.rs"));
        accept(
            &mut preview,
            "a.rs",
            1,
            Ok(doc("a.rs", &["fn main() {}", ""])),
        );
        preview.scroll_line = 1;
        let generation = preview.target(PathBuf::from("b.rs"));
        assert_eq!(generation, 2);
        assert_eq!(preview.preview, PreviewStatus::Loading);
        assert!(preview.doc.is_none());
        assert_eq!(preview.scroll_line, 0);
        assert_eq!(preview.total_lines, 0);
    }

    #[test]
    fn a_result_from_an_older_generation_is_dropped() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.rs"));
        let accepted = accept(
            &mut preview,
            "a.rs",
            generation - 1,
            Ok(doc("a.rs", &["x"])),
        );
        assert!(!accepted);
        assert_eq!(preview.preview, PreviewStatus::Loading);
    }

    #[test]
    fn a_result_for_another_path_is_dropped() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.rs"));
        let accepted = accept(&mut preview, "b.rs", generation, Ok(doc("b.rs", &["x"])));
        assert!(!accepted);
        assert_eq!(preview.preview, PreviewStatus::Loading);
    }

    #[test]
    fn a_loaded_document_becomes_the_body_and_the_language() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.rs"));
        assert!(accept(
            &mut preview,
            "a.rs",
            generation,
            Ok(doc("a.rs", &["a", "b", "c"]))
        ));
        assert_eq!(preview.preview, PreviewStatus::Ready);
        assert_eq!(preview.total_lines, 3);
        assert_eq!(preview.language, "rust");
        assert_eq!(preview.language_label(), "rust");
    }

    #[test]
    fn a_binary_document_reports_its_kind_in_the_status_bar() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.png"));
        let mut binary = doc("a.png", &[]);
        binary.binary = Some(crate::core::preview::BinaryKind::Image);
        assert!(accept(&mut preview, "a.png", generation, Ok(binary)));
        assert_eq!(preview.language, "image");
    }

    #[test]
    fn a_failed_load_is_shown_without_a_body() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.rs"));
        let err = PreviewError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(accept(&mut preview, "a.rs", generation, Err(err)));
        assert_eq!(preview.preview, PreviewStatus::Failed);
        assert!(preview.doc.is_none());
        assert_eq!(preview.total_lines, 0);
    }

    #[test]
    fn a_reload_keeps_the_scroll_until_the_document_gets_shorter() {
        let mut preview = PreviewState {
            viewport: 10,
            ..Default::default()
        };
        let generation = preview.target(PathBuf::from("a.rs"));
        accept(
            &mut preview,
            "a.rs",
            generation,
            Ok(doc("a.rs", &["x"; 100])),
        );
        preview.scroll_line = 90;
        // Same file, new generation: the position survives the swap.
        let generation = preview.target(PathBuf::from("a.rs"));
        preview.scroll_line = 90;
        accept(
            &mut preview,
            "a.rs",
            generation,
            Ok(doc("a.rs", &["x"; 100])),
        );
        assert_eq!(preview.scroll_line, 90);
        // Now the file is short: the view clamps to the end instead of
        // showing an empty pane.
        let generation = preview.target(PathBuf::from("a.rs"));
        preview.scroll_line = 90;
        accept(&mut preview, "a.rs", generation, Ok(doc("a.rs", &["x"; 3])));
        assert_eq!(preview.scroll_line, 0);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut preview = PreviewState {
            viewport: 10,
            ..Default::default()
        };
        let generation = preview.target(PathBuf::from("a.rs"));
        accept(
            &mut preview,
            "a.rs",
            generation,
            Ok(doc("a.rs", &["x"; 25])),
        );
        preview.scroll_by(-5);
        assert_eq!(preview.scroll_line, 0, "scrolled above the first line");
        preview.scroll_by(100);
        assert_eq!(preview.scroll_line, 15, "did not stop at the last page");
        assert!(preview.at_bottom());
        preview.scroll_by(1);
        assert_eq!(preview.scroll_line, 15);
    }

    #[test]
    fn a_shorter_pane_pulls_the_view_back() {
        let mut preview = PreviewState {
            viewport: 10,
            ..Default::default()
        };
        let generation = preview.target(PathBuf::from("a.rs"));
        accept(
            &mut preview,
            "a.rs",
            generation,
            Ok(doc("a.rs", &["x"; 25])),
        );
        preview.scroll_line = 15;
        preview.viewport = 20;
        preview.clamp();
        assert_eq!(preview.scroll_line, 5, "the last page is now shorter");
    }

    #[test]
    fn horizontal_scrolling_follows_the_widest_line() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.rs"));
        accept(
            &mut preview,
            "a.rs",
            generation,
            Ok(doc("a.rs", &["ab", "abcd"])),
        );
        // The pane is two columns wide, so the four column line can start one
        // column to the left and still fill it.
        assert_eq!(preview.widest_line(), 4);
        assert_eq!(preview.max_scroll_col(2), 2);
        preview.scroll_by_columns(2, 2);
        assert_eq!(preview.scroll_col, 2);
        preview.scroll_by_columns(100, 2);
        assert_eq!(preview.scroll_col, 2, "cannot scroll past the widest line");
        preview.scroll_by_columns(-10, 2);
        assert_eq!(preview.scroll_col, 0);
    }

    #[test]
    fn a_narrow_document_cannot_be_scrolled_sideways() {
        let mut preview = PreviewState::default();
        let generation = preview.target(PathBuf::from("a.rs"));
        accept(&mut preview, "a.rs", generation, Ok(doc("a.rs", &["ab"])));
        assert_eq!(preview.max_scroll_col(80), 0);
        preview.scroll_by_columns(5, 80);
        assert_eq!(preview.scroll_col, 0);
    }

    #[test]
    fn clearing_forgets_the_target() {
        let mut preview = PreviewState::default();
        preview.target(PathBuf::from("a.rs"));
        preview.clear();
        assert_eq!(preview.preview, PreviewStatus::Empty);
        assert!(preview.path.is_none());
        assert!(preview.doc.is_none());
    }

    /// The body goes while the read for the other kind of view is on its
    /// way; the file and the line stay, so the view is where it was once
    /// the right body arrives (spec §6).
    #[test]
    fn dropping_the_body_keeps_the_file_and_the_line() {
        let mut preview = shown();
        preview.scroll_line = 7;

        preview.drop_body();

        assert!(preview.doc.is_none());
        assert_eq!(preview.preview, PreviewStatus::Loading);
        assert_eq!(preview.path, Some(PathBuf::from("a.rs")));
        assert_eq!(preview.scroll_line, 7);
        assert_eq!(preview.total_lines, 100, "the size is still known");
    }

    /// One hit, as a run reports it.
    fn hit(rel: &str, line: u32) -> GrepMatch {
        GrepMatch {
            rel: PathBuf::from(rel),
            line,
            text: "let n = 42;".to_owned(),
            submatches: vec![(8, 10)],
        }
    }

    /// A pane showing a hundred lines of `a.rs`.
    fn shown() -> PreviewState {
        let mut preview = PreviewState {
            viewport: 10,
            ..Default::default()
        };
        preview.target(PathBuf::from("a.rs"));
        accept(&mut preview, "a.rs", 1, Ok(doc("a.rs", &vec!["line"; 100])));
        preview
    }

    #[test]
    fn the_line_of_a_hit_comes_to_the_middle_once_its_body_is_there() {
        let mut preview = shown();
        preview.aim_at(Some(&hit("a.rs", 50)));
        // The mark lands where the body is: line 50 with five rows above it.
        assert_eq!(preview.scroll_line, 44);
        assert_eq!(
            preview.marks().map(|focus| focus.line),
            Some(50),
            "the pane says what it is lit on"
        );
    }

    #[test]
    fn the_same_hit_again_leaves_where_the_reader_has_scrolled_to() {
        let mut preview = shown();
        preview.aim_at(Some(&hit("a.rs", 50)));
        preview.scroll_line = 7;
        preview.aim_at(Some(&hit("a.rs", 50)));
        assert_eq!(preview.scroll_line, 7, "nothing moved");
    }

    #[test]
    fn a_mark_is_only_read_on_the_file_it_belongs_to() {
        let mut preview = shown();
        preview.target(PathBuf::from("b.rs"));
        preview.aim_at(Some(&hit("a.rs", 3)));
        assert!(preview.marks().is_none(), "another file is showing");
        preview.aimed_at(Path::new("b.rs"));
        assert!(preview.focus.is_none(), "the mark went with the file");
    }

    #[test]
    fn aiming_at_nothing_takes_the_mark_away() {
        let mut preview = shown();
        preview.aim_at(Some(&hit("a.rs", 3)));
        preview.aim_at(None);
        assert!(preview.focus.is_none());
        assert!(preview.marks().is_none());
    }

    #[test]
    fn clearing_takes_the_mark_and_anything_still_owed_with_it() {
        let mut preview = PreviewState {
            viewport: 10,
            ..Default::default()
        };
        preview.target(PathBuf::from("a.rs"));
        preview.aim_at(Some(&hit("a.rs", 50)));
        preview.clear();
        assert!(preview.focus.is_none());
        // The body comes back for the file, and there is no line left to
        // bring to the middle of it.
        accept(&mut preview, "a.rs", 1, Ok(doc("a.rs", &vec!["line"; 100])));
        assert_eq!(preview.scroll_line, 0);
    }
}
