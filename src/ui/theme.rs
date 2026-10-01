//! Semantic colors mapped onto the 16 ANSI colors.
//!
//! Nothing in the UI picks a color directly; views ask for a [`Role`] and the
//! theme decides. True color is never emitted so that the user's own terminal
//! palette stays in charge. With `NO_COLOR` the role degrades to bold, dim and
//! reverse only.

use ratatui::style::{Color, Modifier, Style};

/// What a piece of text means, not how it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    PaneBorder,
    PaneTitle,
    TreeText,
    TreeDirectory,
    TreeSymlink,
    TreeUnreadable,
    TreeSelected,
    PreviewText,
    PreviewHeader,
    PreviewPlaceholder,
    PreviewGutter,
    StatusMode,
    StatusMessage,
    StatusError,
    StatusInfo,
    StatusPosition,
    StatusMuted,
    TooSmall,
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
        Role::PaneTitle | Role::TreeText | Role::PreviewText | Role::StatusMessage => Color::Reset,
        Role::TreeDirectory => Color::Blue,
        Role::TreeSymlink => Color::Cyan,
        Role::TreeUnreadable | Role::StatusError => Color::Red,
        Role::TreeSelected => Color::Reset,
        Role::PreviewHeader | Role::StatusInfo => Color::Yellow,
        Role::PreviewPlaceholder | Role::PreviewGutter => Color::DarkGray,
        Role::StatusMode => Color::Black,
        Role::StatusPosition => Color::Reset,
    }
}

/// The attributes a role uses, which are all that is left when `NO_COLOR` is
/// set.
fn modifiers(role: Role) -> Modifier {
    match role {
        Role::PaneTitle => Modifier::BOLD,
        Role::TreeSelected => Modifier::REVERSED | Modifier::BOLD,
        Role::TreeDirectory => Modifier::BOLD,
        Role::StatusMode => Modifier::REVERSED | Modifier::BOLD,
        Role::StatusError => Modifier::BOLD,
        Role::StatusMuted
        | Role::PreviewPlaceholder
        | Role::PreviewGutter
        | Role::TooSmall
        | Role::PaneBorder => Modifier::DIM,
        Role::StatusInfo
        | Role::StatusMessage
        | Role::StatusPosition
        | Role::TreeText
        | Role::TreeSymlink
        | Role::TreeUnreadable
        | Role::PreviewText
        | Role::PreviewHeader => Modifier::empty(),
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
            Role::TreeDirectory,
            Role::StatusError,
            Role::StatusInfo,
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

    #[test]
    fn no_color_keeps_attributes_but_drops_colors() {
        let colored = Theme::new(false);
        let plain = Theme::new(true);
        for role in [Role::TreeDirectory, Role::StatusError, Role::StatusMode] {
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
}
