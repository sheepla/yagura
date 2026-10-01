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
            (IconStyle::Text, NodeKind::Dir) => "d",
            (IconStyle::Text, NodeKind::File) => "f",
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
    fn text_icons_are_ascii_and_one_column_wide() {
        let icons = Icons::new(IconStyle::Text);
        for kind in [
            NodeKind::File,
            NodeKind::Dir,
            NodeKind::Symlink,
            NodeKind::Other,
        ] {
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
    fn directories_are_told_apart_from_files_in_both_styles() {
        for style in [IconStyle::Text, IconStyle::Nerd] {
            let icons = Icons::new(style);
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
}
