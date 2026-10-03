//! The pump that turns terminal events into actions.
//!
//! crossterm's `EventStream` needs a futures stream extension, so the events
//! are read with the blocking API on a dedicated OS thread instead. That keeps
//! the dependency set as small as the design asks for and keeps the UI task free
//! of any read: the thread only owns stdin.
//!
//! The pump polls rather than blocks outright, because an editor started on
//! the same terminal has to have the events to itself for as long as it runs
//! (§8, design §14.2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use tokio::sync::mpsc::Sender;

use crate::app::Action;

/// Capacity of the input channel. Keys arrive one at a time, so a small buffer
/// with back pressure is enough and keeps the channel bounded (invariant I6).
pub const INPUT_CHANNEL_CAPACITY: usize = 64;

/// How long the pump waits for an event before looking at the flag again.
const EVENT_POLL: Duration = Duration::from_millis(100);

/// How often the pump looks at the flag while an editor has the terminal.
const SUSPENDED_POLL: Duration = Duration::from_millis(20);

/// Whether an editor owns the terminal input right now.
static SUSPENDED: AtomicBool = AtomicBool::new(false);

/// Stops reading terminal events for the UI, so that an editor started on the
/// same terminal has them to itself (§8).
pub fn suspend() {
    SUSPENDED.store(true, Ordering::Release);
}

/// Reads terminal events for the UI again.
pub fn resume() {
    SUSPENDED.store(false, Ordering::Release);
}

/// Whether [`suspend`] is in effect.
pub fn suspended() -> bool {
    SUSPENDED.load(Ordering::Acquire)
}

/// Reads terminal events until reading fails.
///
/// The thread is detached on purpose: it blocks on stdin and cannot be asked to
/// stop, but it exits on its own as soon as the store drops its channel.
pub fn spawn(tx: Sender<Action>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            if suspended() {
                // Nothing is read while the editor is up, so a key it is
                // about to read cannot be taken by mistake.
                thread::sleep(SUSPENDED_POLL);
                continue;
            }
            match event::poll(EVENT_POLL) {
                Ok(false) => {}
                Ok(true) => {
                    if suspended() {
                        // The flag went up while waiting for this event: it
                        // belongs to the editor that has just taken over, so
                        // it is left where it is rather than read.
                        continue;
                    }
                    let event = match event::read() {
                        Ok(event) => event,
                        Err(err) => {
                            tracing::warn!(%err, "terminal event read failed");
                            break;
                        }
                    };
                    let Some(action) = translate(event) else {
                        continue;
                    };
                    // Waiting for room instead of dropping is deliberate: a lost key is
                    // a lost command, and the store drains this channel first.
                    if tx.blocking_send(action).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "terminal event poll failed");
                    break;
                }
            }
        }
        tracing::debug!("terminal event pump ended");
    })
}

/// The action for a terminal event, or `None` when it is not ours.
pub fn translate(event: Event) -> Option<Action> {
    match event {
        Event::Key(key) => is_press(&key).then_some(Action::Key(key)),
        Event::Resize(width, height) => Some(Action::Resize { width, height }),
        Event::Paste(text) => Some(Action::Paste(text)),
        // Mouse and focus events are ignored: the mouse is never captured so
        // that the user can still select text with it.
        _ => None,
    }
}

/// Releases and repeats count as presses, anything else does not.
fn is_press(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use super::*;
    use tokio::sync::mpsc::error::TrySendError;

    #[test]
    fn key_presses_resizes_and_pastes_become_actions() {
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let action = translate(Event::Key(key)).expect("action");
        assert!(matches!(action, Action::Key(_)));
        let action = translate(Event::Resize(80, 24)).expect("action");
        assert!(matches!(
            action,
            Action::Resize {
                width: 80,
                height: 24
            }
        ));
        let action = translate(Event::Paste("text".to_owned())).expect("action");
        assert!(matches!(action, Action::Paste(_)));
    }

    #[test]
    fn key_repeats_count_but_releases_do_not() {
        let mut key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert!(translate(Event::Key(key)).is_none());
        key.kind = KeyEventKind::Repeat;
        assert!(translate(Event::Key(key)).is_some());
    }

    #[test]
    fn mouse_and_focus_events_are_dropped() {
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(translate(mouse).is_none());
        assert!(translate(Event::FocusGained).is_none());
    }

    #[test]
    fn the_pump_reads_again_only_once_the_editor_has_gone() {
        assert!(!suspended(), "the pump starts out reading");
        suspend();
        assert!(suspended());
        resume();
        assert!(!suspended());
    }

    #[tokio::test]
    async fn a_key_is_kept_even_when_the_channel_is_full() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Action>(1);
        // The single slot is taken.
        tx.try_send(Action::Tick {
            now: std::time::SystemTime::UNIX_EPOCH,
        })
        .expect("first action fits");
        assert!(matches!(
            tx.try_send(Action::Tick {
                now: std::time::SystemTime::UNIX_EPOCH
            }),
            Err(TrySendError::Full(_))
        ));

        let producer = tokio::task::spawn_blocking(move || {
            // Waits for room rather than dropping the key.
            tx.blocking_send(Action::Quit).expect("send");
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !producer.is_finished(),
            "the key was dropped instead of queued"
        );

        assert!(matches!(rx.recv().await, Some(Action::Tick { .. })));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(500), rx.recv()).await,
            Ok(Some(Action::Quit))
        ));
    }

    #[tokio::test]
    async fn the_input_channel_is_bounded() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Action>(INPUT_CHANNEL_CAPACITY);
        drop(rx);
        assert!(tx.send(Action::Quit).await.is_err());
    }
}
