//! Turning process signals into a quit action.

use std::time::Duration;

use tokio::sync::mpsc::Sender;

use super::restore_stdout;
use crate::app::Action;

/// How long the store gets to shut down before the terminal is restored anyway.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(200);

/// Resolves when the process is asked to terminate, then asks the store to quit.
///
/// The restore sequence is emitted by the store when it drops its guard. If the
/// store does not react in time - it is stuck in a blocking call - the terminal
/// is restored here so the shell is never left in raw mode.
pub async fn watch_for_quit(tx: Sender<Action>) {
    wait_for_signal().await;
    tracing::debug!("termination signal received");
    if tokio::time::timeout(SHUTDOWN_GRACE, tx.send(Action::Quit))
        .await
        .is_err()
    {
        restore_stdout();
    }
}

/// Resolves on `SIGTERM` / `SIGHUP` on unix and on `Ctrl+C` everywhere.
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(%err, "cannot listen for SIGTERM");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut hangup = match signal(SignalKind::hangup()) {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(%err, "cannot listen for SIGHUP");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = hangup.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
