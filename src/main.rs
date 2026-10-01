//! yagura - a TUI file tree watcher for observing what a coding agent is doing.

use std::io;

use anyhow::Context;
use clap::Parser;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use yagura::app::{Action, AppState, runner::EffectRunner, store};
use yagura::cli::{Cli, Config};
use yagura::input::task;
use yagura::terminal::{self, TerminalGuard};

fn main() -> anyhow::Result<()> {
    let config = Config::from_args(Cli::parse())?;
    init_logging(config.log.as_deref())?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;

    runtime.block_on(run(config))
}

async fn run(config: Config) -> anyhow::Result<()> {
    // Argument errors happen before the terminal is touched, so a bad command
    // line never leaves the user's shell in a strange state.
    let size = ratatui::crossterm::terminal::size().unwrap_or((80, 24));
    let state = AppState::new(config, size);

    let mut guard =
        TerminalGuard::enter(&mut io::stdout()).context("cannot set up the terminal")?;
    terminal::install_panic_hook();
    tracing::debug!(root = ?state.root(), "yagura started");

    let backend = CrosstermBackend::new(io::stdout());
    let mut app_store =
        store::Store::with_sync(state, backend, true).context("cannot create the terminal")?;

    let (action_tx, action_rx) = mpsc::channel::<Action>(1024);
    let (input_tx, input_rx) = mpsc::channel::<Action>(task::INPUT_CHANNEL_CAPACITY);
    let _input = task::spawn(input_tx.clone());
    let _signals = tokio::spawn(terminal::signals::watch_for_quit(input_tx.clone()));

    let runner = EffectRunner::new(
        app_store.state.root().to_path_buf(),
        app_store.state.config.ignore_enabled,
        action_tx.clone(),
    );
    for effect in app_store.state.startup_effects() {
        runner.run(effect);
    }

    let result = store::run(&mut app_store, input_rx, action_rx, &runner)
        .await
        .context("the terminal stopped working");

    guard
        .restore(&mut io::stdout())
        .context("cannot restore the terminal")?;

    if app_store.state.config.selftest_panic {
        // Deliberate: the panic hook has to bring the terminal back.
        panic!("selftest panic requested with --selftest-panic");
    }
    result
}

/// Logs to a file, never to the terminal: stray output would corrupt the
/// alternate screen the UI draws into.
fn init_logging(path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let Some(path) = path
        .map(std::path::Path::to_path_buf)
        .or_else(log_path_from_env)
    else {
        return Ok(());
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    tracing_subscriber::fmt()
        .with_writer(file)
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .init();
    tracing::info!(log = %path.display(), "logging to file");
    Ok(())
}

fn log_path_from_env() -> Option<std::path::PathBuf> {
    let value = std::env::var_os("YAGURA_LOG")?;
    (!value.is_empty()).then(|| std::path::PathBuf::from(value))
}
