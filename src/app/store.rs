//! The store loop: actions in, state changed, frame out.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::mpsc::Receiver;

use super::action::Action;
use super::effect::Effect;
use super::reduce::reduce;
use super::runner::EffectRunner;
use super::state::AppState;
use crate::core::text;
use crate::ui::FrameSync;

/// Shortest time between two frames. Ratatui only writes the cells that
/// changed, so this is a ceiling on how often a busy screen is repainted.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// What can go wrong while drawing. The store stays backend agnostic, so the
/// backend error is kept as a message instead of boxing a type it cannot name.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("terminal io error: {0}")]
    Io(#[from] io::Error),
    #[error("terminal backend error: {0}")]
    Backend(String),
}

/// State plus the terminal it is drawn on.
pub struct Store<B: Backend> {
    pub state: AppState,
    terminal: Terminal<B>,
    sync: FrameSync,
    last_draw: Option<Instant>,
    dirty: bool,
    /// How many frames have been drawn, which the tests count.
    pub draw_count: u64,
}

impl<B: Backend> Store<B> {
    fn backend_error(err: B::Error) -> StoreError {
        StoreError::Backend(err.to_string())
    }

    /// Wraps a backend. Synchronized output is enabled for the real terminal
    /// and disabled where it cannot help.
    pub fn new(state: AppState, backend: B) -> Result<Self, StoreError> {
        Self::with_sync(state, backend, false)
    }

    pub fn with_sync(state: AppState, backend: B, sync: bool) -> Result<Self, StoreError> {
        Ok(Self {
            terminal: Terminal::new(backend).map_err(Self::backend_error)?,
            state,
            sync: FrameSync::new(sync),
            last_draw: None,
            dirty: true,
            draw_count: 0,
        })
    }

    /// Applies an action and reports the effects to run.
    pub fn handle(&mut self, action: Action) -> Vec<Effect> {
        let repaint = self.state.wants_repaint(&action);
        let effects = reduce(&mut self.state, action);
        if repaint {
            self.dirty = true;
        }
        effects
    }

    /// True when the state changed and the frame interval has passed.
    pub fn frame_due(&self) -> bool {
        self.dirty
            && self
                .last_draw
                .is_none_or(|last| last.elapsed() >= FRAME_INTERVAL)
    }

    /// Draws a frame. Only reads the state.
    pub fn draw(&mut self) -> Result<(), StoreError> {
        self.sync.begin()?;
        let result = self
            .terminal
            .draw(|frame| super::view::render(frame, &self.state))
            .map(|_| ())
            .map_err(Self::backend_error);
        self.sync.end()?;
        self.last_draw = Some(Instant::now());
        self.dirty = false;
        self.draw_count += 1;
        result
    }

    /// Draws when a frame is due.
    pub fn draw_if_due(&mut self) -> Result<bool, StoreError> {
        if !self.frame_due() {
            return Ok(false);
        }
        self.draw()?;
        Ok(true)
    }

    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }

    /// Forgets the screen it has drawn.
    ///
    /// An editor has the terminal for a while and gives it back with whatever
    /// it left on it, so no cell the store remembers can be assumed to still
    /// be there (§8): the next frame is drawn over the whole screen.
    pub fn invalidate(&mut self) {
        let _ = self.terminal.clear();
        self.dirty = true;
        self.last_draw = None;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// Why the loop woke up, held in one value so that the channels can be
/// borrowed again as soon as an action is in hand.
enum Wake {
    Input(Option<Action>),
    Action(Option<Action>),
    Frame,
}

/// Runs the loop until a quit action arrives.
///
/// Input has its own channel so that `select!` can drain it first: a key press
/// is never stuck behind a queue of background results.
pub async fn run<B: Backend>(
    store: &mut Store<B>,
    mut input_rx: Receiver<Action>,
    mut action_rx: Receiver<Action>,
    runner: &EffectRunner,
) -> Result<(), StoreError> {
    let mut frame_timer = tokio::time::interval(FRAME_INTERVAL);
    frame_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick fires immediately, which draws the first frame.
    loop {
        let wake = tokio::select! {
            biased;
            action = input_rx.recv() => Wake::Input(action),
            action = action_rx.recv() => Wake::Action(action),
            _ = frame_timer.tick() => Wake::Frame,
        };
        match wake {
            Wake::Frame => {}
            Wake::Input(None) | Wake::Action(None) => break,
            Wake::Input(Some(action)) | Wake::Action(Some(action)) => {
                apply(store, action, runner, &mut input_rx, &mut action_rx).await?;
            }
        }
        store.draw_if_due()?;
        if store.state.quitting {
            return Ok(());
        }
    }
    store.draw_if_due()?;
    Ok(())
}

/// Applies one action and runs what it asks for.
///
/// An editor is the one effect the runner cannot take: the terminal has to be
/// given back for the child and taken back after it, and that happens here,
/// where both channels are within reach (design §14.2).
async fn apply<B: Backend>(
    store: &mut Store<B>,
    action: Action,
    runner: &EffectRunner,
    input_rx: &mut Receiver<Action>,
    action_rx: &mut Receiver<Action>,
) -> Result<(), StoreError> {
    for effect in store.handle(action) {
        match effect {
            Effect::RunEditor { path, line } => {
                edit(store, path, line, input_rx, action_rx, runner).await?;
            }
            effect => runner.run(effect),
        }
    }
    Ok(())
}

/// Opens an editor with the terminal given back to it first (§8, design
/// §14.2):
///
/// 1. the input pump stops reading, so the child has the keys to itself;
/// 2. the alternate screen and raw mode go away, the child runs, and the
///    filesystem work carries on while no frame is drawn;
/// 3. the terminal comes back and the screen is redrawn from scratch;
/// 4. the pump reads again.
async fn edit<B: Backend>(
    store: &mut Store<B>,
    path: PathBuf,
    line: Option<usize>,
    input_rx: &mut Receiver<Action>,
    action_rx: &mut Receiver<Action>,
    runner: &EffectRunner,
) -> Result<(), StoreError> {
    let invocation = crate::core::editor::resolve(&path, line);
    crate::input::task::suspend();
    if let Err(err) = crate::terminal::pause(&mut io::stdout()) {
        crate::input::task::resume();
        return Err(err.into());
    }
    let outcome = match tokio::process::Command::new(invocation.program())
        .args(invocation.args())
        .spawn()
    {
        Ok(mut child) => wait_while_working(&mut child, store, runner, action_rx).await,
        Err(err) => Err(err),
    };
    let terminal_back = crate::terminal::resume(&mut io::stdout());
    store.invalidate();
    drain_keys(store, runner, input_rx);
    crate::input::task::resume();
    terminal_back?;
    if let Err(err) = outcome {
        store.state.set_error(cannot_open(&path, &err));
    }
    Ok(())
}

/// What the status bar says when the editor will not start (§8).
///
/// The path is the user's, so it reaches the terminal through the same
/// sanitizing as every other text.
fn cannot_open(path: &Path, err: &io::Error) -> String {
    format!(
        "cannot open {}: {}",
        text::sanitize_os(path.as_os_str()),
        text::os_error(err)
    )
}

/// Waits for the child while the state keeps up with the filesystem; no frame
/// is drawn until the editor is gone (design §14.2).
async fn wait_while_working<B: Backend>(
    child: &mut tokio::process::Child,
    store: &mut Store<B>,
    runner: &EffectRunner,
    action_rx: &mut Receiver<Action>,
) -> io::Result<()> {
    loop {
        tokio::select! {
            status = child.wait() => return status.map(|_| ()),
            action = action_rx.recv() => match action {
                Some(action) => {
                    for effect in store.handle(action) {
                        // An editor cannot be opened while another one has
                        // the terminal, and nothing else asks for one; the
                        // runner leaves the effect alone (design §14.2).
                        runner.run(effect);
                    }
                }
                // Nothing will be sent again, so only the child can end the
                // wait now.
                None => return child.wait().await.map(|_| ()),
            }
        }
    }
}

/// Drops the keys that were already on their way to the panes when the editor
/// took the terminal: they were meant for it, and answering them here would
/// make one press mean two things (§8).
fn drain_keys<B: Backend>(
    store: &mut Store<B>,
    runner: &EffectRunner,
    input_rx: &mut Receiver<Action>,
) {
    while let Ok(action) = input_rx.try_recv() {
        match action {
            Action::Key(_) | Action::Paste(_) => {}
            action => {
                for effect in store.handle(action) {
                    runner.run(effect);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::command::Command;
    use crate::app::state::{Config, Focus, PreviewMode};
    use crate::cli::{BaselineKind, IconStyle};

    fn store(width: u16, height: u16) -> Store<TestBackend> {
        let config = Config {
            root: PathBuf::from("/root"),
            icons: IconStyle::Text,
            baseline: BaselineKind::Auto,
            ignore_enabled: true,
            no_color: false,
            log: None,
            selftest_panic: false,
        };
        Store::new(
            AppState::new(config, (width, height)),
            TestBackend::new(width, height),
        )
        .expect("store")
    }

    #[test]
    fn an_editor_that_will_not_start_says_why_with_the_path_sanitized() {
        let message = cannot_open(
            Path::new("/root/weird\u{1b}[2J.rs"),
            &io::Error::from(io::ErrorKind::NotFound),
        );
        assert_eq!(message, "cannot open /root/weird\u{241b}[2J.rs: not found");
    }

    #[test]
    fn an_idle_store_does_not_draw() {
        let mut store = store(80, 24);
        store.draw().expect("first frame");
        assert_eq!(store.draw_count, 1);
        for _ in 0..100 {
            assert!(!store.draw_if_due().expect("draw check"));
        }
        assert_eq!(store.draw_count, 1);
    }

    #[test]
    fn the_frame_interval_holds_back_a_second_frame() {
        let mut store = store(80, 24);
        store.draw().expect("first frame");
        store.dirty = true;
        assert!(!store.frame_due(), "drew again immediately");
        std::thread::sleep(FRAME_INTERVAL + Duration::from_millis(5));
        assert!(store.frame_due());
        store.draw_if_due().expect("draw");
        assert_eq!(store.draw_count, 2);
    }

    #[test]
    fn handling_an_action_marks_the_state_dirty() {
        let mut store = store(80, 24);
        store.draw().expect("first frame");
        store.handle(Action::Command(Command::ToggleTree));
        assert!(store.is_dirty());
        assert!(!store.state.tree_visible);
    }

    #[test]
    fn a_clock_tick_alone_does_not_ask_for_a_frame() {
        let mut store = store(80, 24);
        store.draw().expect("first frame");
        store.handle(Action::Tick {
            now: std::time::SystemTime::UNIX_EPOCH,
        });
        assert!(!store.is_dirty(), "the clock caused a repaint");
        assert_eq!(store.draw_count, 1);
    }

    #[test]
    fn commands_reach_the_state() {
        let mut store = store(80, 24);
        store.handle(Action::Command(Command::CyclePreviewMode));
        assert_eq!(store.state.preview_mode, PreviewMode::InlineDiff);
        store.handle(Action::Command(Command::ToggleFocus));
        assert_eq!(store.state.focus, Focus::Preview);
        store.handle(Action::Command(Command::Quit));
        assert!(store.state.quitting);
    }

    #[tokio::test]
    async fn the_loop_runs_effects_and_stops_on_quit() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let config = Config {
            root: dir.path().to_path_buf(),
            icons: IconStyle::Text,
            baseline: BaselineKind::Auto,
            ignore_enabled: true,
            no_color: false,
            log: None,
            selftest_panic: false,
        };
        let mut store =
            Store::new(AppState::new(config, (80, 24)), TestBackend::new(80, 24)).expect("store");
        let (action_tx, action_rx) = tokio::sync::mpsc::channel(64);
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(64);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, action_tx.clone());
        for effect in store.state.startup_effects() {
            runner.run(effect);
        }

        let quit = tokio::spawn(async move {
            // Let the root listing arrive before quitting.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = input_tx.send(Action::Quit).await;
        });
        run(&mut store, input_rx, action_rx, &runner)
            .await
            .expect("loop");
        quit.await.expect("quit task");
        assert!(store.state.quitting);
        assert!(store.draw_count >= 1);
    }

    #[tokio::test]
    async fn the_loop_ends_when_the_input_channel_closes() {
        let mut store = store(80, 24);
        let (_action_tx, action_rx) = tokio::sync::mpsc::channel(4);
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(4);
        drop(input_tx);
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let runner = EffectRunner::new(PathBuf::from("/root"), true, tx);
        run(&mut store, input_rx, action_rx, &runner)
            .await
            .expect("loop");
    }
}
