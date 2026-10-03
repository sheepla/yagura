//! Icon glyphs.
//!
//! `text` sticks to single width ASCII so that rows stay aligned in terminals
//! without a patched font; `nerd` uses Nerd Font code points.

use crate::cli::IconStyle;
use crate::core::fsmodel::NodeKind;

/// Marker of a row that has children and is expanded.
pub const EXPANDED: &str = "▾";
/// Marker of a row that has children and is collapsed.
pub const COLLAPSED: &str = "▸";
/// Marker column of a row without children.
pub const LEAF: &str = " ";

#[derive(Clone, Copy, Debug)]
pub struct Icons {
    style: IconStyle,
}

impl Icons {
    pub fn new(style: IconStyle) -> Self {
        Self { style }
    }

    pub fn style(&self) -> IconStyle {
        self.style
    }

    pub fn for_kind(&self, kind: NodeKind) -> &'static str {
        match (self.style, kind) {
            // A text row carries no letter for a file or a directory: the
            // marker already tells the two apart (§11.1).
            (IconStyle::Text, NodeKind::Dir | NodeKind::File) => "",
            (IconStyle::Text, NodeKind::Symlink) => "l",
            (IconStyle::Text, NodeKind::Other) => "o",
            (IconStyle::Nerd, NodeKind::Dir) => "\u{f07b}",
            (IconStyle::Nerd, NodeKind::File) => "\u{f15b}",
            (IconStyle::Nerd, NodeKind::Symlink) => "\u{f0c1}",
            (IconStyle::Nerd, NodeKind::Other) => "\u{f15b}",
        }
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    #[test]
    fn text_rows_carry_no_letter_for_a_file_or_a_directory() {
        let icons = Icons::new(IconStyle::Text);
        assert_eq!(icons.for_kind(NodeKind::Dir), "");
        assert_eq!(icons.for_kind(NodeKind::File), "");
        // The letters that still say something stay one column wide.
        for kind in [NodeKind::Symlink, NodeKind::Other] {
            let icon = icons.for_kind(kind);
            assert!(icon.is_ascii(), "{kind:?} icon {icon:?} is not ASCII");
            assert_eq!(UnicodeWidthStr::width(icon), 1);
        }
        assert_eq!(icons.style(), IconStyle::Text);
    }

    #[test]
    fn nerd_icons_are_private_use_glyphs() {
        let icons = Icons::new(IconStyle::Nerd);
        for kind in [
            NodeKind::File,
            NodeKind::Dir,
            NodeKind::Symlink,
            NodeKind::Other,
        ] {
            let icon = icons.for_kind(kind);
            let code = icon.chars().next().expect("non empty") as u32;
            assert!(
                (0xF000..=0xF2FF).contains(&code),
                "{kind:?} icon {icon:?} is not a Nerd Font glyph"
            );
        }
    }

    #[test]
    fn nerd_icons_tell_a_directory_from_a_file() {
        let icons = Icons::new(IconStyle::Nerd);
        assert_ne!(
            icons.for_kind(NodeKind::Dir),
            icons.for_kind(NodeKind::File)
        );
        assert_ne!(
            icons.for_kind(NodeKind::Symlink),
            icons.for_kind(NodeKind::File)
        );
    }
}
