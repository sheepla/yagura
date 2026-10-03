//! Semantic colors mapped onto the 16 ANSI colors.
//!
//! Nothing in the UI picks a color directly; views ask for a [`Role`] and the
//! theme decides. True color is never emitted so that the user's own terminal
//! palette stays in charge. With `NO_COLOR` the role degrades to bold, dim and
//! reverse only.

use ratatui::style::{Color, Modifier, Style};

use crate::core::fsmodel::Status;

/// What a piece of text means, not how it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    PaneBorder,
    /// The frame of the pane that holds the focus, so it stands apart from
    /// the dim frame of the pane that does not.
    PaneBorderFocused,
    PaneTitle,
    TreeText,
    TreeDirectory,
    TreeSymlink,
    /// The marker of a directory that is open, which must stay findable
    /// next to the dim frame around it.
    TreeExpanded,
    TreeUnreadable,
    TreeSelected,
    /// The age of a change that is seconds old, at the right end of a row.
    TreeFresh,
    PreviewText,
    PreviewHeader,
    PreviewPlaceholder,
    PreviewGutter,
    /// A heading of a markdown document (§9.4).
    PreviewHeading,
    /// Emphasis or strong emphasis in markdown.
    PreviewEmphasis,
    /// A quoted line in markdown.
    PreviewQuote,
    /// A line inside a markdown code block.
    PreviewCodeBlock,
    /// `code` inside a markdown line.
    PreviewInlineCode,
    /// The text of a markdown link, with no address behind it.
    PreviewLink,
    /// Markdown kept as it was written: tables, images, rules (§9.4).
    PreviewDim,
    /// The part of a preview line a grep hit lands on (§13.3).
    MatchHighlight,
    /// A line only the current file has, in a diff.
    DiffAdded,
    /// A line only the baseline has, in a diff.
    DiffRemoved,
    /// A file changed against the baseline (`M`).
    StatusModified,
    /// A file the baseline does not have (`A`).
    StatusAdded,
    /// A file only the baseline has (`D`).
    StatusDeleted,
    StatusMode,
    StatusMessage,
    StatusError,
    StatusInfo,
    StatusPosition,
    StatusMuted,
    TooSmall,
    /// A word the language is built on: `let`, `if`, an operator.
    SyntaxKeyword,
    /// The contents of a quoted string.
    SyntaxString,
    /// A remark left for a reader.
    SyntaxComment,
    /// A number written out.
    SyntaxNumber,
    /// A name of a type.
    SyntaxType,
    /// The name a function is called by.
    SyntaxFunction,
}

impl Role {
    /// The role a baseline status is drawn in, in a tree row and in the
    /// preview header.
    pub fn for_status(status: Status) -> Role {
        match status {
            Status::Unchanged => Role::TreeText,
            Status::Modified => Role::StatusModified,
            Status::Added => Role::StatusAdded,
            Status::Deleted => Role::StatusDeleted,
        }
    }
}

/// Resolves roles to styles for one terminal.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    no_color: bool,
}

impl Theme {
    pub fn new(no_color: bool) -> Self {
        Self { no_color }
    }

    /// Reads `NO_COLOR` from the environment.
    pub fn from_env() -> Self {
        Self::new(std::env::var_os("NO_COLOR").is_some())
    }

    pub fn is_no_color(&self) -> bool {
        self.no_color
    }

    /// The style for a role.
    pub fn style(&self, role: Role) -> Style {
        let modifiers = modifiers(role);
        if self.no_color {
            return Style::default().add_modifier(modifiers);
        }
        Style::default().fg(color(role)).add_modifier(modifiers)
    }
}

/// The 16 color a role uses.
fn color(role: Role) -> Color {
    match role {
        Role::PaneBorder | Role::StatusMuted | Role::TooSmall => Color::DarkGray,
        Role::PaneBorderFocused => Color::White,
        Role::TreeExpanded => Color::White,
        Role::PaneTitle | Role::TreeText | Role::PreviewText | Role::StatusMessage => Color::Reset,
        Role::TreeDirectory => Color::Blue,
        Role::TreeSymlink => Color::Cyan,
        Role::TreeUnreadable | Role::StatusError | Role::StatusDeleted => Color::Red,
        Role::TreeSelected | Role::MatchHighlight => Color::Reset,
        Role::PreviewHeader | Role::StatusInfo | Role::StatusModified | Role::TreeFresh => {
            Color::Yellow
        }
        Role::PreviewPlaceholder
        | Role::PreviewGutter
        | Role::PreviewCodeBlock
        | Role::PreviewQuote
        | Role::PreviewDim => Color::DarkGray,
        Role::PreviewHeading => Color::Blue,
        Role::PreviewEmphasis => Color::Reset,
        Role::PreviewInlineCode => Color::Green,
        Role::PreviewLink => Color::Cyan,
        Role::DiffAdded | Role::StatusAdded => Color::Green,
        Role::DiffRemoved => Color::Red,
        Role::StatusMode => Color::Black,
        Role::StatusPosition => Color::Reset,
        Role::SyntaxKeyword => Color::Magenta,
        Role::SyntaxString => Color::Green,
        Role::SyntaxComment => Color::DarkGray,
        Role::SyntaxNumber => Color::Cyan,
        Role::SyntaxType => Color::Yellow,
        Role::SyntaxFunction => Color::Blue,
    }
}

/// The attributes a role uses, which are all that is left when `NO_COLOR` is
/// set.
fn modifiers(role: Role) -> Modifier {
    match role {
        Role::PaneTitle | Role::PreviewHeading | Role::PreviewEmphasis => Modifier::BOLD,
        Role::PreviewLink => Modifier::UNDERLINED,
        Role::TreeSelected | Role::MatchHighlight => Modifier::REVERSED | Modifier::BOLD,
        Role::TreeDirectory => Modifier::BOLD,
        Role::TreeFresh => Modifier::BOLD,
        Role::StatusMode => Modifier::REVERSED | Modifier::BOLD,
        Role::StatusError => Modifier::BOLD,
        Role::StatusMuted
        | Role::PreviewPlaceholder
        | Role::PreviewGutter
        | Role::PreviewCodeBlock
        | Role::PreviewQuote
        | Role::PreviewDim
        | Role::TooSmall
        | Role::PaneBorder
        | Role::SyntaxComment => Modifier::DIM,
        Role::StatusInfo
        | Role::StatusMessage
        | Role::StatusPosition
        | Role::TreeText
        | Role::TreeSymlink
        | Role::TreeUnreadable
        | Role::PreviewText
        | Role::PreviewHeader
        | Role::DiffAdded
        | Role::DiffRemoved
        | Role::StatusModified
        | Role::StatusAdded
        | Role::StatusDeleted
        | Role::SyntaxKeyword
        | Role::SyntaxString
        | Role::SyntaxNumber
        | Role::SyntaxType
        | Role::SyntaxFunction
        | Role::PreviewInlineCode
        | Role::PaneBorderFocused
        | Role::TreeExpanded => Modifier::empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_from_the_ansi_16_palette() {
        let theme = Theme::new(false);
        for role in [
            Role::PaneBorder,
            Role::PaneBorderFocused,
            Role::TreeExpanded,
            Role::TreeDirectory,
            Role::StatusError,
            Role::StatusInfo,
            Role::SyntaxKeyword,
            Role::SyntaxString,
            Role::SyntaxComment,
            Role::SyntaxNumber,
            Role::SyntaxType,
            Role::SyntaxFunction,
            Role::PreviewHeading,
            Role::PreviewQuote,
            Role::PreviewInlineCode,
            Role::PreviewLink,
            Role::MatchHighlight,
        ] {
            let style = theme.style(role);
            let color = style.fg.expect("a color");
            assert!(
                matches!(
                    color,
                    Color::Reset
                        | Color::Black
                        | Color::Red
                        | Color::Green
                        | Color::Yellow
                        | Color::Blue
                        | Color::Magenta
                        | Color::Cyan
                        | Color::Gray
                        | Color::DarkGray
                        | Color::LightRed
                        | Color::LightGreen
                        | Color::LightYellow
                        | Color::LightBlue
                        | Color::LightMagenta
                        | Color::LightCyan
                        | Color::White
                ),
                "{role:?} used a color outside the 16 ANSI colors"
            );
        }
    }

    /// Six meanings shown in one pane must not look like one another.
    #[test]
    fn each_syntax_color_is_its_own() {
        let theme = Theme::new(false);
        let roles = [
            Role::SyntaxKeyword,
            Role::SyntaxString,
            Role::SyntaxComment,
            Role::SyntaxNumber,
            Role::SyntaxType,
            Role::SyntaxFunction,
        ];
        let colors: Vec<_> = roles
            .iter()
            .map(|role| theme.style(*role).fg.expect("a color"))
            .collect();
        for (index, color) in colors.iter().enumerate() {
            assert!(
                !colors[..index].contains(color),
                "{:?} was used twice",
                roles[index]
            );
        }
    }

    #[test]
    fn no_color_keeps_attributes_but_drops_colors() {
        let colored = Theme::new(false);
        let plain = Theme::new(true);
        for role in [
            Role::TreeDirectory,
            Role::StatusError,
            Role::StatusMode,
            Role::SyntaxComment,
        ] {
            let with_color = colored.style(role);
            let without_color = plain.style(role);
            assert!(with_color.fg.is_some());
            assert!(without_color.fg.is_none(), "{role:?} still had a color");
            assert_eq!(
                with_color.add_modifier, without_color.add_modifier,
                "{role:?} lost its attributes under NO_COLOR"
            );
        }
        assert!(plain.is_no_color());
        assert!(!colored.is_no_color());
    }

    #[test]
    fn selection_is_reversible_in_both_modes() {
        for theme in [Theme::new(false), Theme::new(true)] {
            let style = theme.style(Role::TreeSelected);
            assert!(style.add_modifier.contains(Modifier::REVERSED));
        }
    }

    /// The focused frame is the bright one, the other is dim: that is how
    /// the pane with the focus is told apart from the one without it.
    #[test]
    fn the_focused_frame_stands_apart_from_the_other() {
        let theme = Theme::new(false);
        let focused = theme.style(Role::PaneBorderFocused);
        let other = theme.style(Role::PaneBorder);
        assert_eq!(focused.fg, Some(Color::White));
        assert_eq!(other.fg, Some(Color::DarkGray));
        assert!(!focused.add_modifier.intersects(Modifier::DIM));
        assert!(other.add_modifier.contains(Modifier::DIM));
        // Under NO_COLOR the two still differ, by their attributes.
        let plain = Theme::new(true);
        assert_ne!(
            plain.style(Role::PaneBorderFocused).add_modifier,
            plain.style(Role::PaneBorder).add_modifier,
            "NO_COLOR cannot tell the focused frame from the other"
        );
    }
}
