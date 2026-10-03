//! Building the command that opens a file in an external editor (§8).
//!
//! The value of `$VISUAL`/`$EDITOR` is split the way a shell would, so that
//! `code -w` runs `code` with `-w` rather than hunting for one program of that
//! whole name. No shell is ever involved: the words go straight to the child
//! (§3).

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// One editor to run: the program, and the words that go before the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    program: OsString,
    args: Vec<OsString>,
}

impl Invocation {
    /// The program to run, which is the first word of the configured value.
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// Everything after it, line argument and file included.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }
}

/// Editors that take the line as `+N` in front of the file (§8).
const PLUS_LINE_EDITORS: &[&str] = &["vi", "vim", "nvim", "view", "ex", "nano", "emacs"];

/// Editors that take the line as part of the file word, `file:N` (§8).
const COLON_LINE_EDITORS: &[&str] = &["hx", "helix"];

/// What to open with: `$VISUAL`, then `$EDITOR`, then `vi` (§8).
///
/// A variable that holds nothing but spaces counts as unset, and having
/// neither of them is not a failure: `vi` is what the specification falls
/// back to.
pub fn resolve(path: &Path, line: Option<usize>) -> Invocation {
    let configured = nonempty_env("VISUAL").or_else(|| nonempty_env("EDITOR"));
    build(
        configured.unwrap_or_else(|| OsString::from("vi")),
        path,
        line,
    )
}

fn nonempty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.to_string_lossy().trim().is_empty())
}

/// Turns the whole configured value into a program and its arguments.
fn build(configured: OsString, path: &Path, line: Option<usize>) -> Invocation {
    // A value that is not valid text is taken as one word rather than taken
    // apart: splitting bytes that are not characters would change them.
    let mut words = match configured.to_str() {
        Some(value) => split(value),
        None => vec![configured],
    };
    if words.is_empty() {
        words.push(OsString::from("vi"));
    }
    let program = words.remove(0);
    let mut args = words;
    let file = match (line, line_style(&program)) {
        (Some(number), LineStyle::Colon) => {
            OsString::from(format!("{}:{number}", path.to_string_lossy()))
        }
        (Some(number), LineStyle::Plus) => {
            args.push(OsString::from(format!("+{number}")));
            path.as_os_str().to_owned()
        }
        (_, _) => path.as_os_str().to_owned(),
    };
    args.push(file);
    Invocation { program, args }
}

/// How an editor would like to be told a line number (§8).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineStyle {
    /// `+N` before the file.
    Plus,
    /// `file:N`, the file and the line in one word.
    Colon,
    /// An editor the table does not know: it is only ever given the file.
    Unknown,
}

fn line_style(program: &OsStr) -> LineStyle {
    let name = editor_name(program);
    if PLUS_LINE_EDITORS.contains(&name.as_str()) {
        LineStyle::Plus
    } else if COLON_LINE_EDITORS.contains(&name.as_str()) {
        LineStyle::Colon
    } else {
        LineStyle::Unknown
    }
}

/// The editor a word points at: the last part of the path, lowercased, and
/// without the suffix Windows gives an executable.
fn editor_name(program: &OsStr) -> String {
    let file = Path::new(program).file_name().unwrap_or(program);
    let mut name = file.to_string_lossy().to_lowercase();
    for suffix in [".exe", ".bat", ".cmd", ".com"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            name = stem.to_owned();
            break;
        }
    }
    name
}

/// Splits a configured value into words the way a shell would, by hand:
/// whitespace separates, either kind of quote keeps a word together, and a
/// backslash in front of a quote, a whitespace or another backslash escapes
/// it.
///
/// Every other backslash is kept as it is, so a Windows path such as
/// `C:\vim\vim.exe` comes through whole.
pub fn split(value: &str) -> Vec<OsString> {
    let mut words: Vec<OsString> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(OsString::from(std::mem::take(&mut word)));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                started = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"') | Some('\\')) => {
                            word.push(chars.next().expect("the peek saw it"));
                        }
                        c => word.push(c),
                    }
                }
            }
            '\\' => match chars.peek() {
                Some(&next)
                    if next == '\'' || next == '"' || next == '\\' || next.is_whitespace() =>
                {
                    chars.next();
                    started = true;
                    word.push(next);
                }
                _ => {
                    started = true;
                    word.push('\\');
                }
            },
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(OsString::from(word));
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(configured: &str, file: &str, line: Option<usize>) -> Vec<String> {
        let invocation = build(OsString::from(configured), Path::new(file), line);
        let mut words = vec![invocation.program().to_string_lossy().into_owned()];
        words.extend(
            invocation
                .args()
                .iter()
                .map(|word| word.to_string_lossy().into_owned()),
        );
        words
    }

    #[test]
    fn a_value_with_more_than_one_word_runs_the_first_program_with_the_rest() {
        assert_eq!(
            words("code -w", "src/main.rs", None),
            vec!["code", "-w", "src/main.rs"]
        );
        assert_eq!(
            words("emacs -nw", "src/main.rs", None),
            vec!["emacs", "-nw", "src/main.rs"]
        );
    }

    #[test]
    fn the_file_is_the_last_word_however_the_editor_is_configured() {
        assert_eq!(words("vim", "a.txt", None), vec!["vim", "a.txt"]);
        assert_eq!(words("", "a.txt", None), vec!["vi", "a.txt"]);
        assert_eq!(words("   ", "a.txt", None), vec!["vi", "a.txt"]);
    }

    #[test]
    fn a_line_reaches_the_editors_that_take_it_before_the_file() {
        assert_eq!(words("vim", "a.txt", Some(12)), vec!["vim", "+12", "a.txt"]);
        assert_eq!(words("nvim", "a.txt", Some(3)), vec!["nvim", "+3", "a.txt"]);
        assert_eq!(words("nano", "a.txt", Some(7)), vec!["nano", "+7", "a.txt"]);
        assert_eq!(
            words("emacs -nw", "a.txt", Some(5)),
            vec!["emacs", "-nw", "+5", "a.txt"]
        );
    }

    #[test]
    fn a_line_is_part_of_the_file_word_for_helix() {
        assert_eq!(
            words("hx", "src/main.rs", Some(9)),
            vec!["hx", "src/main.rs:9"]
        );
        assert_eq!(
            words("helix", "src/main.rs", Some(2)),
            vec!["helix", "src/main.rs:2"]
        );
    }

    #[test]
    fn an_editor_the_table_does_not_know_is_only_given_the_file() {
        assert_eq!(words("code", "a.txt", Some(4)), vec!["code", "a.txt"]);
        assert_eq!(words("micro", "a.txt", Some(4)), vec!["micro", "a.txt"]);
    }

    #[test]
    fn an_editor_is_recognised_through_its_suffix_and_case() {
        assert_eq!(
            words("VIM.EXE", "a.txt", Some(2)),
            vec!["VIM.EXE", "+2", "a.txt"]
        );
        assert_eq!(
            words("Emacs", "a.txt", Some(2)),
            vec!["Emacs", "+2", "a.txt"]
        );
        assert_eq!(
            words("C:/tools/vim.bat", "a.txt", Some(2)),
            vec!["C:/tools/vim.bat", "+2", "a.txt"]
        );
    }

    #[test]
    fn words_split_where_a_shell_would_split_them() {
        assert_eq!(
            split("code -w"),
            vec![OsString::from("code"), OsString::from("-w")]
        );
        assert_eq!(
            split("  nano   -t  "),
            vec![OsString::from("nano"), OsString::from("-t")]
        );
        assert!(split("").is_empty());
        assert!(split("   ").is_empty());
    }

    #[test]
    fn quotes_keep_the_spaces_inside_one_word() {
        assert_eq!(
            split("'my editor' -f"),
            vec![OsString::from("my editor"), OsString::from("-f")]
        );
        assert_eq!(
            split("\"my editor\" -f"),
            vec![OsString::from("my editor"), OsString::from("-f")]
        );
        assert_eq!(split("''"), vec![OsString::from("")]);
    }

    #[test]
    fn a_backslash_escapes_but_a_windows_path_survives() {
        assert_eq!(split("a\\ b"), vec![OsString::from("a b")]);
        assert_eq!(
            split("C:\\vim\\vim.exe"),
            vec![OsString::from("C:\\vim\\vim.exe")]
        );
        assert_eq!(
            split("\"C:\\Program Files\\vim\\vim.exe\""),
            vec![OsString::from("C:\\Program Files\\vim\\vim.exe")]
        );
    }

    #[test]
    fn an_unterminated_quote_takes_the_rest_of_the_value() {
        assert_eq!(split("'my editor"), vec![OsString::from("my editor")]);
        assert_eq!(split("\"my editor"), vec![OsString::from("my editor")]);
    }
}
