//! Text helpers for everything that reaches the terminal.
//!
//! Strings that originate outside the process - file contents, file names,
//! `rg` output, error messages - must pass through [`sanitize`] before they
//! are drawn, otherwise control sequences in the data could move the cursor,
//! clear the screen or reorder text (invariant I5).

use std::ffi::OsStr;
use std::io;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Columns a tab is expanded to.
pub const TAB_WIDTH: usize = 4;

/// First code point of the Unicode "Control Pictures" block.
const CONTROL_PICTURE_BASE: u32 = 0x2400;

/// Describes an operating system error in English.
///
/// The `Display` of an [`io::Error`] comes from the operating system and is
/// translated into the language of the machine, so a message that goes to the
/// user is built from the [`io::ErrorKind`] and the number instead.
pub fn os_error(err: &io::Error) -> String {
    let reason = match err.kind() {
        io::ErrorKind::NotFound => "not found",
        io::ErrorKind::PermissionDenied => "permission denied",
        io::ErrorKind::AlreadyExists => "already exists",
        io::ErrorKind::InvalidInput => "invalid input",
        io::ErrorKind::InvalidData => "invalid data",
        io::ErrorKind::UnexpectedEof => "unexpected end of file",
        io::ErrorKind::TimedOut => "timed out",
        io::ErrorKind::Interrupted => "interrupted",
        io::ErrorKind::IsADirectory => "is a directory",
        io::ErrorKind::NotADirectory => "not a directory",
        io::ErrorKind::Unsupported => "not supported",
        io::ErrorKind::OutOfMemory => "out of memory",
        _ => "failed",
    };
    match err.raw_os_error() {
        Some(code) => format!("{reason} (os error {code})"),
        None => reason.to_owned(),
    }
}

/// Replaces the characters that could confuse the terminal with visible ones.
///
/// `\r\n` becomes `\n`, tabs are expanded to [`TAB_WIDTH`] spaces, other C0
/// controls become control pictures (`ESC` renders as `U+241B` "␛"), `DEL`
/// becomes "␡", C1 controls become the replacement character and bidirectional
/// formatting characters become a visible `⟪U+202E⟫` label.
pub fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    out.push('\n');
                    chars.next();
                } else {
                    push_control_picture(&mut out, ch);
                }
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str(&" ".repeat(TAB_WIDTH)),
            '\u{7f}' => out.push('\u{2421}'),
            c if is_c0(c) => push_control_picture(&mut out, c),
            '\u{80}'..='\u{9f}' => out.push(char::REPLACEMENT_CHARACTER),
            c if is_bidi(c) => {
                out.push('\u{27ea}');
                out.push_str(&format!("U+{:04X}", c as u32));
                out.push('\u{27eb}');
            }
            c => out.push(c),
        }
    }
    out
}

/// Sanitizes an operating system string, replacing invalid UTF-8 with the
/// replacement character instead of failing.
pub fn sanitize_os(name: &OsStr) -> String {
    sanitize(&name.to_string_lossy())
}

/// Columns occupied by `text` once drawn.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Truncates `text` to at most `max` columns, marking the cut with `…`.
pub fn truncate_to_width(text: &str, max: usize) -> String {
    let window = window(text, 0, max);
    if !window.cut {
        return text.to_owned();
    }
    let mut out = text[..window.end].to_owned();
    out.push('…');
    out
}

/// Truncates `text` to at most `max` columns from the left, keeping its end
/// and marking the cut with `…` in front of it.
///
/// The tail of a name is the part that tells files apart, so a tree row that
/// has to give up columns gives up the beginning.
pub fn truncate_tail_to_width(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    // The ellipsis is drawn in the first column the kept tail may use.
    let budget = max - 1;
    let mut width = 0;
    let mut start = text.len();
    for (index, grapheme) in text.grapheme_indices(true).rev() {
        width += UnicodeWidthStr::width(grapheme);
        if width > budget {
            break;
        }
        start = index;
    }
    format!("\u{2026}{}", &text[start..])
}

/// The part of `text` a pane that does not wrap can show.
///
/// Byte offsets into `text`, so that a line can be cut once and then sliced by
/// meaning as well as by columns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    /// Byte offset the window starts at, `from` columns into the line.
    pub start: usize,
    /// Byte offset the window ends at.
    pub end: usize,
    /// The line continues past `end`, so a `…` belongs after it.
    pub cut: bool,
}

/// The window of `text` from column `from` that fits in `width` columns.
pub fn window(text: &str, from: usize, width: usize) -> Window {
    let start = skip_offset(text, from);
    let visible = &text[start..];
    if UnicodeWidthStr::width(visible) <= width {
        return Window {
            start,
            end: text.len(),
            cut: false,
        };
    }
    // One column of the pane belongs to the ellipsis.
    let budget = width.saturating_sub(1);
    Window {
        start,
        end: start + fit_offset(visible, budget),
        cut: true,
    }
}

/// Byte offset where the first `max` columns of `text` end.
///
/// A grapheme that does not fit is left out whole, so the offset never lands
/// inside a cluster — an emoji joined by zero width joiners counts as the two
/// columns it draws as, not the sum of its parts.
fn fit_offset(text: &str, max: usize) -> usize {
    let mut width = 0;
    let mut offset = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if width + grapheme_width > max {
            break;
        }
        width += grapheme_width;
        offset = index + grapheme.len();
    }
    offset
}

/// Byte offset where `from` columns of scrolling have been passed.
///
/// A grapheme the window starts in the middle of is kept: dropping half of it
/// would draw garbage where the pane begins.
fn skip_offset(text: &str, from: usize) -> usize {
    let mut width = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        width += UnicodeWidthStr::width(grapheme);
        if width > from {
            return index;
        }
    }
    text.len()
}

fn push_control_picture(out: &mut String, ch: char) {
    let code = CONTROL_PICTURE_BASE + ch as u32;
    // The control pictures block covers every C0 code point and DEL.
    if let Some(picture) = char::from_u32(code) {
        out.push(picture);
    } else {
        out.push(char::REPLACEMENT_CHARACTER);
    }
}

fn is_c0(ch: char) -> bool {
    (ch as u32) < 0x20
}

fn is_bidi(ch: char) -> bool {
    matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ordinary_text_untouched() {
        assert_eq!(sanitize("hello あい"), "hello あい");
    }

    #[test]
    fn normalizes_crlf_and_lone_cr() {
        assert_eq!(sanitize("a\r\nb\rc"), "a\nb\u{240d}c");
    }

    #[test]
    fn expands_tabs_to_four_columns() {
        assert_eq!(sanitize("a\tb"), "a    b");
    }

    #[test]
    fn replaces_escape_and_nul_with_control_pictures() {
        assert_eq!(sanitize("\x1b[31m\0\x07"), "\u{241b}[31m\u{2400}\u{2407}");
    }

    #[test]
    fn replaces_del_and_c1_controls() {
        assert_eq!(sanitize("\u{7f}\u{9b}31m"), "\u{2421}\u{fffd}31m");
    }

    #[test]
    fn makes_bidi_controls_visible() {
        assert_eq!(sanitize("a\u{202e}b"), "a\u{27ea}U+202E\u{27eb}b");
        assert_eq!(sanitize("\u{2066}x"), "\u{27ea}U+2066\u{27eb}x");
        assert_eq!(sanitize("\u{2069}"), "\u{27ea}U+2069\u{27eb}");
    }

    #[test]
    fn sanitize_os_replaces_invalid_utf8() {
        #[cfg(unix)]
        {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            let name = OsString::from_vec(vec![b'a', 0xff]);
            assert_eq!(sanitize_os(&name), "a\u{fffd}");
        }
        #[cfg(not(unix))]
        {
            assert_eq!(sanitize_os(OsStr::new("a")), "a");
        }
    }

    #[test]
    fn measures_east_asian_width() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("あい"), 4);
    }

    #[test]
    fn a_window_cuts_on_emoji_cluster_edges() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let line = format!("a{family}zz");
        // Four columns of window: the cluster is two of them and stays whole.
        let end = window(&line, 0, 4);
        assert_eq!(&line[end.start..end.end], format!("a{family}"));
        // Two columns of scrolling end inside the cluster, so the window
        // starts at its first edge rather than halfway through it.
        let start = window(&line, 2, 4);
        assert_eq!(&line[start.start..start.start + family.len()], family);
    }

    #[test]
    fn a_tail_cut_keeps_its_emoji_cluster_whole() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let name = format!("aa{family}");
        assert_eq!(
            truncate_tail_to_width(&name, 3),
            format!("\u{2026}{family}")
        );
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        assert_eq!(truncate_to_width("abc", 10), "abc");
        assert_eq!(truncate_to_width("あい", 3), "あ…");
        assert_eq!(truncate_to_width("abcdef", 0), "…");
    }
}
