//! Taking the terminal over and giving it back.
//!
//! Every exit path - normal return, panic, `SIGTERM`, `SIGHUP` - ends in the
//! same restore sequence (invariant I10).

pub mod signals;

use std::io::{self, Write};

use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

/// Escape sequences that put the terminal into the state the UI needs.
const INIT_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?2004h\x1b[?25l";

/// The exact inverse of [`INIT_SEQUENCE`].
const RESTORE_SEQUENCE: &[u8] = b"\x1b[?25h\x1b[?2004l\x1b[?1049l";

/// Restores the terminal on drop and on demand.
///
/// The guard remembers whether it still owns the terminal, so calling
/// [`TerminalGuard::restore`] twice writes the restore sequence once.
pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    /// Switches to the alternate screen and turns raw mode on.
    pub fn enter<W: Write>(writer: &mut W) -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = write_all(writer, INIT_SEQUENCE) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self { active: true })
    }

    /// Gives the terminal back. Later calls do nothing.
    pub fn restore<W: Write>(&mut self, writer: &mut W) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        write_all(writer, RESTORE_SEQUENCE)?;
        disable_raw_mode()
    }

    /// True while the guard still owns the terminal.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// A guard that owns nothing, for tests that only exercise the sequences.
    #[cfg(test)]
    pub(crate) fn detached(active: bool) -> Self {
        Self { active }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            restore_stdout();
        }
    }
}

/// Restores the terminal without a guard.
///
/// Used by the panic hook and the signal handler, which run outside the store
/// loop and cannot reach its guard. Writing the sequence twice is harmless.
pub fn restore_stdout() {
    let mut stdout = io::stdout();
    let _ = write_all(&mut stdout, RESTORE_SEQUENCE);
    let _ = disable_raw_mode();
}

/// Hands the terminal over to a child process for as long as it runs (§8,
/// design §14.2): raw mode comes off, the alternate screen goes away, and the
/// cursor comes back so the child draws where the reader can see it.
pub fn pause<W: Write>(writer: &mut W) -> io::Result<()> {
    let _ = disable_raw_mode();
    write_all(writer, RESTORE_SEQUENCE)
}

/// Takes the terminal back after the child has gone. The screen behind the
/// sequence holds whatever the child left there, so the caller has to redraw
/// the whole frame rather than only what changed.
pub fn resume<W: Write>(writer: &mut W) -> io::Result<()> {
    enable_raw_mode()?;
    write_all(writer, INIT_SEQUENCE)
}

/// Installs a panic hook that restores the terminal before the default hook
/// prints the panic message.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_stdout();
        previous(info);
    }));
}

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::cursor::{Hide, Show};
    use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};

    /// The same sequence written with crossterm commands, so the test can prove
    /// the hand written bytes match what crossterm would send.
    fn write_init_with_commands<W: Write>(writer: &mut W) -> io::Result<()> {
        execute!(writer, EnterAlternateScreen, EnableBracketedPaste, Hide)
    }

    fn write_restore_with_commands<W: Write>(writer: &mut W) -> io::Result<()> {
        execute!(writer, Show, DisableBracketedPaste, LeaveAlternateScreen)
    }

    #[test]
    fn init_and_restore_sequences_are_inverses() {
        fn has(haystack: &[u8], needle: &[u8]) -> bool {
            haystack
                .windows(needle.len())
                .any(|window| window == needle)
        }
        assert!(has(RESTORE_SEQUENCE, b"\x1b[?1049l"));
        assert!(has(INIT_SEQUENCE, b"\x1b[?1049h"));
        assert!(has(INIT_SEQUENCE, b"\x1b[?2004h"));
        assert!(has(RESTORE_SEQUENCE, b"\x1b[?2004l"));
        assert!(has(INIT_SEQUENCE, b"\x1b[?25l"));
        assert!(has(RESTORE_SEQUENCE, b"\x1b[?25h"));
    }

    /// The half of the editor handover that needs no console: the sequence
    /// that gives the terminal to the child (§8, design §14.2).
    #[test]
    fn a_pause_hands_the_terminal_over_to_the_child() {
        let mut buffer: Vec<u8> = Vec::new();
        pause(&mut buffer).expect("pause");
        assert_eq!(
            buffer, RESTORE_SEQUENCE,
            "the child draws on the real screen"
        );
    }

    /// The round trip, which puts the console into raw mode and therefore
    /// needs one: `cargo test -- --ignored` on a real terminal (§12, design
    /// §14.2).
    #[test]
    #[ignore = "needs a console to put into raw mode"]
    fn a_pause_and_a_resume_bring_the_terminal_back() {
        let mut buffer: Vec<u8> = Vec::new();
        pause(&mut buffer).expect("pause");
        resume(&mut buffer).expect("resume");
        assert_eq!(
            buffer,
            [RESTORE_SEQUENCE, INIT_SEQUENCE].concat(),
            "restore first, then the alternate screen again"
        );
    }

    #[test]
    fn restore_is_idempotent() {
        let mut buffer: Vec<u8> = Vec::new();
        let mut guard = TerminalGuard::detached(true);
        assert!(guard.is_active());
        guard.restore(&mut buffer).expect("first restore");
        let after_first = buffer.len();
        guard.restore(&mut buffer).expect("second restore");
        assert!(!guard.is_active());
        assert_eq!(buffer.len(), after_first, "second restore wrote bytes");
    }

    #[test]
    fn an_inactive_guard_writes_nothing() {
        let mut buffer: Vec<u8> = Vec::new();
        let mut guard = TerminalGuard::detached(false);
        guard.restore(&mut buffer).expect("restore");
        assert!(buffer.is_empty());
    }

    #[test]
    fn crossterm_commands_match_the_byte_sequence() {
        let mut from_commands: Vec<u8> = Vec::new();
        write_init_with_commands(&mut from_commands).expect("commands");
        assert_eq!(from_commands, INIT_SEQUENCE);

        let mut from_commands: Vec<u8> = Vec::new();
        write_restore_with_commands(&mut from_commands).expect("commands");
        assert_eq!(from_commands, RESTORE_SEQUENCE);
    }
}
