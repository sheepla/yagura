//! The store loop: actions in, state changed, frame out.

use std::io;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::mpsc::Receiver;

use super::action::Action;
use super::effect::Effect;
use super::reduce::reduce;
use super::runner::EffectRunner;
use super::state::AppState;
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

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
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
        tokio::select! {
            biased;
            action = input_rx.recv() => {
                let Some(action) = action else { break };
                dispatch(store, action, runner);
            }
            action = action_rx.recv() => {
                let Some(action) = action else { break };
                dispatch(store, action, runner);
            }
            _ = frame_timer.tick() => {}
        }
        store.draw_if_due()?;
        if store.state.quitting {
            return Ok(());
        }
    }
    store.draw_if_due()?;
    Ok(())
}

fn dispatch<B: Backend>(store: &mut Store<B>, action: Action, runner: &EffectRunner) {
    for effect in store.handle(action) {
        runner.run(effect);
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
