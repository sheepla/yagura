//! Composition root of the state: one field per concern, one place to change.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub use crate::cli::Config;
use crate::core::baseline::{ChangedEntry, Kind, Phase};
use crate::core::fsmodel::NodeId;
use crate::features::finder::FinderState;
use crate::features::help::HelpState;
use crate::features::preview::PreviewState;
use crate::features::tree::TreeState;
use crate::features::tree::icons::Icons;
use crate::features::watch::WatchState;
use crate::ui::theme::Theme;

use super::effect::Effect;

/// How long a non error message stays on the status bar.
pub const MESSAGE_TTL: Duration = Duration::from_secs(3);

/// Which pane receives keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Tree,
    Preview,
}

/// How the preview shows the difference to the baseline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewMode {
    Content,
    InlineDiff,
    SideBySide,
}

impl PreviewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Content => "NORMAL",
            Self::InlineDiff => "DIFF",
            Self::SideBySide => "SPLIT",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Content => Self::InlineDiff,
            Self::InlineDiff => Self::SideBySide,
            Self::SideBySide => Self::Content,
        }
    }

    pub fn is_diff(self) -> bool {
        !matches!(self, Self::Content)
    }
}

/// The modal that currently owns the screen. At most one at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    None,
    Help,
    Files,
    Grep,
    Watch,
}

impl Overlay {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Help => Some("HELP"),
            Self::Files => Some("FILES"),
            Self::Grep => Some("GREP"),
            Self::Watch => Some("WATCH"),
        }
    }

    /// Whether the overlay is one of the two that are a text box with a list
    /// below it, which share their keys and their state (§13).
    pub fn is_finder(self) -> bool {
        matches!(self, Self::Files | Self::Grep)
    }
}

/// Sibling order inside the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    /// Directories first, then natural order.
    Name,
    /// Directories first, then most recently changed.
    Recent,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageKind {
    Info,
    Error,
}

/// The status bar message.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub kind: MessageKind,
    /// `None` keeps the message until the next key press.
    expires_at: Option<SystemTime>,
}

impl Message {
    /// A message that disappears on its own.
    pub fn info(text: impl Into<String>, now: SystemTime) -> Self {
        Self {
            text: text.into(),
            kind: MessageKind::Info,
            expires_at: Some(now + MESSAGE_TTL),
        }
    }

    /// A message that stays until the user presses a key.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: MessageKind::Error,
            expires_at: None,
        }
    }

    pub fn is_visible(&self, now: SystemTime) -> bool {
        self.expires_at.is_none_or(|deadline| now < deadline)
    }
}

/// What the tree and preview know about the baseline.
#[derive(Debug)]
pub struct BaselineState {
    /// The baseline the user is currently looking at.
    pub kind: Kind,
    pub phase: Phase,
    /// Number of the newest build; a result carrying another one is stale.
    pub ver: u64,
    /// True once the root is known to sit in a git work tree; gates `B`.
    pub repo: bool,
    /// `HEAD` of the last build, for the header line.
    pub head_oid: Option<String>,
    /// Every file that differs from the baseline, relative to the root.
    pub changed: BTreeMap<PathBuf, ChangedEntry>,
}

impl BaselineState {
    /// The state before the first build answers.
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            phase: Phase::Loading,
            ver: 1,
            repo: false,
            head_oid: None,
            changed: BTreeMap::new(),
        }
    }
}

/// The single source of truth for the UI.
pub struct AppState {
    pub config: Config,
    pub size: (u16, u16),
    pub focus: Focus,
    pub preview_mode: PreviewMode,
    pub overlay: Overlay,
    pub tree_visible: bool,
    pub follow: bool,
    /// Time of the most recent tick, so that views never read the clock.
    pub now: SystemTime,
    pub message: Option<Message>,
    pub tree: TreeState,
    pub preview: PreviewState,
    /// The query box, list and matcher of the finder overlays (§13).
    pub finder: FinderState,
    /// Where the reader has scrolled to in the help list (§5).
    pub help: HelpState,
    /// The log of changes under the root, kept whether or not it is shown
    /// (§9, §12).
    pub watch: WatchState,
    pub baseline: BaselineState,
    /// Generation of the newest stats request per path; an answer carrying
    /// another one belongs to a request that has been superseded (I3).
    pub stats_gen: BTreeMap<PathBuf, u64>,
    /// Set once a quit action has been handled.
    pub quitting: bool,
    /// Immutable rendering decisions derived from the configuration.
    pub theme: Theme,
    pub icons: Icons,
}

impl AppState {
    pub fn new(config: Config, size: (u16, u16)) -> Self {
        let theme = Theme::new(config.no_color);
        let icons = Icons::new(config.icons);
        let model = crate::core::fsmodel::FsModel::new(config.root.clone());
        // `Auto` starts out as the git baseline; the first build answers
        // whether there is a repository and falls back if there is not.
        let kind = match config.baseline {
            crate::cli::BaselineKind::Snapshot => Kind::Snapshot,
            crate::cli::BaselineKind::Auto | crate::cli::BaselineKind::Head => Kind::Head,
        };
        let mut state = Self {
            tree: TreeState::new(model),
            preview: PreviewState::default(),
            finder: FinderState::default(),
            help: HelpState::default(),
            watch: WatchState::default(),
            baseline: BaselineState::new(kind),
            stats_gen: BTreeMap::new(),
            theme,
            icons,
            config,
            size,
            focus: Focus::Tree,
            preview_mode: PreviewMode::Content,
            overlay: Overlay::None,
            tree_visible: true,
            follow: false,
            now: SystemTime::UNIX_EPOCH,
            message: None,
            quitting: false,
        };
        // The root is expanded so that its listing is visible as soon as it
        // arrives, and the load is registered before it is asked for: the
        // reducer drops a listing whose generation it has not handed out
        // (invariant I3).
        let root = state.tree.model.root_id();
        state.tree.expand(root);
        state.tree.loading.insert(root);
        state.tree.load_gen.insert(root, 1);
        // The state starts out consistent, so that a comparison of two states
        // never reports the lazy row cache as a difference.
        state.tree.set_viewport(state.tree_viewport_height());
        state.preview.viewport = state.preview_viewport_height();
        state.tree.refresh();
        state
    }

    /// Rows the preview body has, which is what a page of scrolling is.
    ///
    /// An open finder draws a preview pane of its own, and a page of the pane
    /// on screen is what a page of scrolling means (§13.2).
    pub fn preview_viewport_height(&self) -> usize {
        let pane = if self.overlay.is_finder() {
            crate::ui::layout::finder(self.area()).preview.height
        } else {
            crate::ui::layout::preview_height(self.area())
        };
        pane.max(1) as usize
    }

    /// The effects the store has to run once at startup.
    pub fn startup_effects(&self) -> Vec<Effect> {
        let root = self.tree.model.root_id();
        // The generation is read back from the tree instead of written out
        // here, so the two cannot drift apart.
        let generation = self.tree.load_gen.get(&root).copied().unwrap_or(1);
        vec![
            // The watch goes up before the first read, so that a change
            // during it cannot slip through (§7.1).
            Effect::WatchDirs {
                add: vec![PathBuf::new()],
                remove: Vec::new(),
            },
            Effect::LoadDir {
                node: root,
                generation,
                rel: PathBuf::new(),
            },
            Effect::ScheduleTick,
            Effect::BuildBaseline {
                kind: self.baseline.kind,
                ver: self.baseline.ver,
            },
        ]
    }

    /// The node the user is working on.
    pub fn selected_node(&self) -> Option<NodeId> {
        self.tree.selected
    }

    /// The absolute path of the selected node.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected_node()
            .and_then(|node| self.tree.model.absolute(node))
    }

    pub fn root(&self) -> &std::path::Path {
        &self.config.root
    }

    /// Replaces the message.
    pub fn set_message(&mut self, message: Message) {
        self.message = Some(message);
    }

    pub fn set_info(&mut self, text: impl Into<String>) {
        let now = self.now;
        self.set_message(Message::info(text, now));
    }

    pub fn set_error(&mut self, text: impl Into<String>) {
        self.set_message(Message::error(text));
    }

    /// The message to show, dropping it once it has expired.
    pub fn visible_message(&self) -> Option<&Message> {
        let now = self.now;
        self.message
            .as_ref()
            .filter(|message| message.is_visible(now))
    }

    /// Drops a sticky message; called when the user presses a key.
    pub fn clear_sticky_message(&mut self) {
        if self
            .message
            .as_ref()
            .is_some_and(|message| message.kind == MessageKind::Error)
        {
            self.message = None;
        }
    }

    /// Rows available to the tree pane for the current terminal size.
    pub fn tree_viewport_height(&self) -> u16 {
        let height = crate::ui::layout::tree_height(self.area(), self.tree_visible);
        if crate::ui::layout::too_small(self.area()) {
            0
        } else {
            height
        }
    }

    /// The full terminal as a rectangle at the origin.
    pub fn area(&self) -> ratatui::layout::Rect {
        ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1)
    }

    /// Which keymap applies right now.
    pub fn context(&self) -> crate::input::keymap::Context {
        use crate::input::keymap::Context;
        match self.overlay {
            Overlay::None => match self.focus {
                Focus::Tree => Context::Tree,
                Focus::Preview => Context::Preview,
            },
            Overlay::Help => Context::Help,
            Overlay::Files | Overlay::Grep => Context::Finder,
            Overlay::Watch => Context::Watch,
        }
    }

    /// Whether an action needs a new frame.
    ///
    /// A clock tick only repaints when something on screen would read
    /// differently: the elapsed times are what a tick changes, and a frame
    /// nobody can tell from the last one is both wasted work and visible as
    /// flicker (design §14.1). Two things are exempt. A `Paste` lands in the
    /// query box of an open finder, which is something to see; and a match
    /// still running keeps asking for the frame its workers are fed from
    /// (§13.2).
    pub fn wants_repaint(&self, action: &crate::app::Action) -> bool {
        match action {
            crate::app::Action::Tick { now } => {
                self.elapsed_tick_changes(*now)
                    || self.message_tick_changes(*now)
                    || (self.overlay.is_finder() && self.finder.matcher_running())
            }
            crate::app::Action::Paste(_) => self.overlay.is_finder(),
            _ => true,
        }
    }

    /// True when a row in view would say something else after this tick.
    fn elapsed_tick_changes(&self, next: SystemTime) -> bool {
        if !self.tree_visible {
            return false;
        }
        let start = self.tree.scroll.min(self.tree.visible.len());
        let end = (start + self.tree.viewport.max(1)).min(self.tree.visible.len());
        self.tree.visible[start..end].iter().any(|row| {
            let Some(node) = self.tree.model.get(row.node) else {
                return false;
            };
            let Some(changed_at) = node.changed_at else {
                return false;
            };
            crate::ui::elapsed::label(changed_at, self.now)
                != crate::ui::elapsed::label(changed_at, next)
        })
    }

    /// True when a message drawn right now would be gone after this tick.
    fn message_tick_changes(&self, next: SystemTime) -> bool {
        let Some(message) = &self.message else {
            return false;
        };
        message.is_visible(self.now) && !message.is_visible(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::IconStyle;

    fn state() -> AppState {
        let config = Config {
            root: PathBuf::from("/root"),
            icons: IconStyle::Text,
            baseline: crate::cli::BaselineKind::Auto,
            ignore_enabled: true,
            no_color: false,
            log: None,
            selftest_panic: false,
        };
        AppState::new(config, (80, 24))
    }

    #[test]
    fn info_messages_expire_but_errors_do_not() {
        let now = SystemTime::UNIX_EPOCH;
        let info = Message::info("hi", now);
        assert!(info.is_visible(now + Duration::from_secs(1)));
        assert!(!info.is_visible(now + MESSAGE_TTL + Duration::from_secs(1)));
        let error = Message::error("boom");
        assert!(error.is_visible(now + Duration::from_secs(3600)));
    }

    #[test]
    fn key_presses_clear_sticky_messages_only() {
        let mut state = state();
        state.set_error("boom");
        state.clear_sticky_message();
        assert!(state.message.is_none());

        state.set_info("hi");
        state.clear_sticky_message();
        assert!(state.message.is_some());
    }

    #[test]
    fn preview_mode_cycles_through_three_states() {
        let mut mode = PreviewMode::Content;
        mode = mode.next();
        assert_eq!(mode, PreviewMode::InlineDiff);
        mode = mode.next();
        assert_eq!(mode, PreviewMode::SideBySide);
        assert_eq!(mode.next(), PreviewMode::Content);
        assert_eq!(PreviewMode::Content.label(), "NORMAL");
        assert!(!PreviewMode::Content.is_diff());
        assert!(PreviewMode::SideBySide.is_diff());
    }

    #[test]
    fn overlay_labels_match_the_status_bar_spec() {
        assert_eq!(Overlay::Help.label(), Some("HELP"));
        assert_eq!(Overlay::None.label(), None);
    }

    #[test]
    fn startup_loads_the_root_and_builds_the_baseline() {
        let state = state();
        let effects = state.startup_effects();
        assert_eq!(effects.len(), 4);
        assert_eq!(
            effects[0],
            Effect::WatchDirs {
                add: vec![PathBuf::new()],
                remove: Vec::new()
            }
        );
        assert_eq!(
            effects[1],
            Effect::LoadDir {
                node: state.tree.model.root_id(),
                generation: 1,
                rel: PathBuf::new()
            }
        );
        assert_eq!(effects[2], Effect::ScheduleTick);
        assert_eq!(
            effects[3],
            Effect::BuildBaseline {
                kind: Kind::Head,
                ver: 1
            }
        );
    }

    #[test]
    fn the_startup_load_is_registered_before_it_is_asked_for() {
        let state = state();
        let root = state.tree.model.root_id();
        // The root is expanded so its listing can be seen at all.
        assert!(state.tree.is_expanded(root));
        // The generation the effect carries is the one the reducer expects,
        // otherwise the listing is dropped as stale when it arrives.
        let Effect::LoadDir { generation, .. } = state.startup_effects()[1] else {
            panic!("startup loads the root");
        };
        assert_eq!(generation, state.tree.load_gen[&root]);
        assert!(state.tree.loading.contains(&root));
    }

    #[test]
    fn tree_viewport_shrinks_when_the_tree_is_hidden() {
        let mut state = state();
        assert_eq!(state.tree_viewport_height(), 21);
        state.tree_visible = false;
        assert_eq!(state.tree_viewport_height(), 0);
    }

    #[test]
    fn a_terminal_that_is_too_small_has_no_tree_viewport() {
        let mut state = state();
        state.size = (40, 10);
        assert_eq!(state.tree_viewport_height(), 0);
    }

    #[test]
    fn a_tick_asks_for_a_frame_only_when_a_row_would_say_something_else() {
        use std::ffi::OsStr;

        use crate::app::Action;
        use crate::core::fsmodel::NodeKind;

        let mut state = state();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(300);
        state.now = now;
        let root = state.tree.model.root_id();
        let file = state
            .tree
            .model
            .insert_child(root, OsStr::new("a.txt"), NodeKind::File);
        state.tree.bump();
        state.tree.refresh();
        let tick = |at: SystemTime| Action::Tick { now: at };

        assert!(
            !state.wants_repaint(&tick(now)),
            "a row nothing happened to has nothing to say"
        );

        // `12s` and `13s` are two words for the same row.
        state.tree.model.get_mut(file).expect("a.txt").changed_at =
            Some(now - Duration::from_secs(12));
        assert!(state.wants_repaint(&tick(now + Duration::from_secs(1))));

        // `3m40s` and `3m41s` are the same word, so no frame is needed.
        state.tree.model.get_mut(file).expect("a.txt").changed_at =
            Some(now - Duration::from_secs(3 * 60 + 40));
        assert!(!state.wants_repaint(&tick(now + Duration::from_secs(1))));

        // With the tree off there is no row to read at all.
        state.tree.model.get_mut(file).expect("a.txt").changed_at =
            Some(now - Duration::from_secs(12));
        state.tree_visible = false;
        assert!(!state.wants_repaint(&tick(now + Duration::from_secs(1))));
    }

    #[test]
    fn a_tick_gives_a_frame_to_a_message_that_has_run_out() {
        use crate::app::Action;

        let mut state = state();
        state.now = SystemTime::UNIX_EPOCH;
        state.set_info("note");
        assert!(
            !state.wants_repaint(&Action::Tick { now: state.now }),
            "the message is still on screen"
        );
        assert!(
            state.wants_repaint(&Action::Tick {
                now: state.now + MESSAGE_TTL + Duration::from_secs(1)
            }),
            "the message is gone but still drawn"
        );
    }

    #[test]
    fn every_action_but_a_bland_paste_asks_for_a_frame() {
        use crate::app::Action;

        let state = state();
        assert!(state.wants_repaint(&Action::Quit));
        assert!(state.wants_repaint(&Action::Resize {
            width: 80,
            height: 24
        }));
        assert!(!state.wants_repaint(&Action::Paste("text".to_owned())));
    }

    #[test]
    fn a_paste_that_lands_in_a_query_box_asks_for_a_frame() {
        use crate::app::Action;

        let mut state = state();
        state.overlay = Overlay::Files;
        assert!(state.wants_repaint(&Action::Paste("text".to_owned())));
    }
}
