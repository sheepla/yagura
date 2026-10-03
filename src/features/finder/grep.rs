//! What a grep run finds, and how one is asked for (§13.3).
//!
//! Running `rg` belongs to the effect runner; what lives here is the data
//! the list draws and the two halves of the conversation with it that are
//! pure: which line of its output is a hit, and which arguments make it
//! speak.

use std::path::{Component, Path, PathBuf};

/// Hits a run reports before it is stopped and the list says so (§7).
pub const MATCH_LIMIT: usize = 1000;

/// Hits carried in one action, so that one busy file costs one message
/// rather than one per line (the walk does the same with its paths).
pub const CHUNK: usize = 64;

/// How long a query waits for the next keystroke before `rg` is asked (§13.3).
pub const DEBOUNCE_MS: u64 = 150;

/// Fewer characters than this is not a search: nothing runs (§7).
pub const MIN_PATTERN: usize = 2;

/// One line `rg` reported hits on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GrepMatch {
    /// Path relative to the root, the way every path is held.
    pub rel: PathBuf,
    /// The line the hits are on, one based, as `--line-number` counts it.
    pub line: u32,
    /// The line as it stands on disk, without its newline.
    pub text: String,
    /// Byte ranges of the hits inside `text`.
    pub submatches: Vec<(usize, usize)>,
}

/// What one overlay of a grep holds in its list (§13.3).
#[derive(Default, Debug)]
pub struct GrepResults {
    /// Run the answers belong to; an answer carrying another one is stale
    /// (invariant I3).
    pub generation: u64,
    /// The hits, in the order `rg` reported them, which groups them by file.
    pub matches: Vec<GrepMatch>,
    /// Whether the run being waited for has spoken, so that its first answer
    /// replaces what the last one left rather than joining it.
    pub answered: bool,
    /// Whether a run is going, which the list header says.
    pub running: bool,
    /// `rg` stopped at the limit, so the list is all there is of it.
    pub truncated: bool,
    /// `rg` could not take the pattern; the list says so instead of hiding it.
    pub error: Option<String>,
    /// `rg` is not installed, which is why the overlay will not open again.
    pub missing: Option<String>,
}

impl GrepResults {
    /// Starts over for a new overlay and returns the run it wants.
    ///
    /// `missing` is kept: it says something about the machine rather than
    /// about one overlay.
    pub fn open(&mut self) -> u64 {
        self.matches.clear();
        self.truncated = false;
        self.error = None;
        self.start_run()
    }

    /// Claims a run of its own: what the last one answered belongs to a
    /// generation nobody is waiting for any more (invariant I3).
    pub fn start_run(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.answered = false;
        self.running = false;
        self.generation
    }

    /// Whether an answer belongs to the run being waited for.
    pub fn answers(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

/// Takes one line of `rg --json` as a hit, or nothing when it is not one.
///
/// Only `match` events carry hits; `begin`, `end` and `summary` say nothing
/// about a line, and a line that is not JSON at all is left alone.
pub fn parse(line: &str) -> Option<GrepMatch> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "match" {
        return None;
    }
    let data = value.get("data")?;
    let rel = data.get("path")?.get("text")?.as_str()?;
    let number = data.get("line_number")?.as_u64()?;
    // The line comes with its terminator still on it, and the pane draws it
    // without one.
    let text = data.get("lines")?.get("text")?.as_str()?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let submatches = data
        .get("submatches")?
        .as_array()?
        .iter()
        .filter_map(|submatch| {
            let start = submatch.get("start")?.as_u64()? as usize;
            let end = submatch.get("end")?.as_u64()? as usize;
            // A hit that runs into the terminator ends where the line ends.
            let start = start.min(text.len());
            let end = end.min(text.len());
            (start < end).then_some((start, end))
        })
        .collect();
    Some(GrepMatch {
        rel: relative(rel),
        line: number as u32,
        text: text.to_owned(),
        submatches,
    })
}

/// The path as every other path in the application holds it (§13.3).
///
/// The search is started in the root and named with a `.`, so `rg` writes
/// what it finds as `./a.txt`; the walk, the tree and the preview all leave
/// that dot out.
fn relative(path: &str) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in Path::new(path).components() {
        if component != Component::CurDir {
            cleaned.push(component);
        }
    }
    cleaned
}

/// The `rg` that answers one query (§13.3).
///
/// The pattern travels as the value of `-e` and the paths follow `--`, so a
/// query that starts with a `-` is a pattern rather than an option of its
/// own, and a query naming a path cannot become one.
pub fn command(pattern: &str, ignore_enabled: bool) -> (String, Vec<String>) {
    let mut args = vec![
        "--json".to_owned(),
        "--line-number".to_owned(),
        "--smart-case".to_owned(),
        "--max-columns".to_owned(),
        "500".to_owned(),
    ];
    if !ignore_enabled {
        args.push("--no-ignore".to_owned());
    }
    args.push("-e".to_owned());
    args.push(pattern.to_owned());
    args.push("--".to_owned());
    args.push(".".to_owned());
    ("rg".to_owned(), args)
}

/// Where the hits land in the line as the pane draws it.
///
/// The offsets `rg` reports count bytes of the line on disk, and the pane
/// draws it after [`crate::core::text::sanitize`] has expanded its tabs and
/// spelled out its control characters, which moves everything behind them.
/// Every offset is therefore put through the same replacement to find where
/// it went (§13.3).
pub fn display_ranges(text: &str, submatches: &[(usize, usize)]) -> Vec<(usize, usize)> {
    submatches
        .iter()
        .filter_map(|&(start, end)| {
            let start = boundary(text, start);
            let end = boundary(text, end);
            (start < end).then(|| {
                (
                    crate::core::text::sanitize(&text[..start]).len(),
                    crate::core::text::sanitize(&text[..end]).len(),
                )
            })
        })
        .collect()
}

/// The offset at or before `at` that still falls between two characters.
fn boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn match_event(rel: &str, number: u64, lines: &str, submatches: &[(&str, u64, u64)]) -> String {
        let submatches: Vec<String> = submatches
            .iter()
            .map(|(text, start, end)| {
                format!(
                    r#"{{"match":{{"text":{text}}},"start":{start},"end":{end}}}"#,
                    text = serde_json::to_string(text).expect("a string")
                )
            })
            .collect();
        format!(
            r#"{{"type":"match","data":{{"path":{{"text":{path}}},"lines":{{"text":{lines}}},"line_number":{number},"absolute_offset":0,"submatches":[{submatches}]}}}}"#,
            path = serde_json::to_string(rel).expect("a string"),
            lines = serde_json::to_string(lines).expect("a string"),
            number = number,
            submatches = submatches.join(","),
        )
    }

    #[test]
    fn a_match_event_becomes_one_row_of_the_list() {
        let line = match_event("src/main.rs", 12, "fn main() {\n", &[("\"main\"", 3, 7)]);
        let hit = parse(&line).expect("a hit");
        assert_eq!(hit.rel, PathBuf::from("src/main.rs"));
        assert_eq!(hit.line, 12);
        assert_eq!(hit.text, "fn main() {", "the terminator is not part of it");
        assert_eq!(hit.submatches, [(3, 7)]);
    }

    #[test]
    fn a_path_the_run_writes_with_a_dot_is_held_the_way_the_walk_holds_it() {
        // The search is started in the root and named with `.`, so the run
        // writes what it finds as `./a.txt` (with the separator of the
        // machine it runs on).
        let dotted = format!(
            ".{}src{}main.rs",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        );
        assert_eq!(relative(&dotted), PathBuf::from("src").join("main.rs"));
        assert_eq!(relative("a.txt"), PathBuf::from("a.txt"));
    }

    #[test]
    fn a_crlf_line_keeps_its_return_out_of_the_row() {
        let line = match_event("a.txt", 1, "one\r\n", &[("\"one\"", 0, 3)]);
        let hit = parse(&line).expect("a hit");
        assert_eq!(hit.text, "one");
        assert_eq!(hit.submatches, [(0, 3)]);
    }

    #[test]
    fn only_a_match_event_carries_a_hit() {
        for kind in ["begin", "end", "summary"] {
            let line = format!(r#"{{"type":"{kind}","data":{{"path":{{"text":"a"}}}}}}"#);
            assert!(parse(&line).is_none(), "{kind} was taken as a hit");
        }
        assert!(parse("not json at all").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn a_hit_past_the_end_of_the_line_is_cut_to_it() {
        let line = match_event("a.txt", 1, "ab\n", &[("\"ab\"", 0, 9)]);
        let hit = parse(&line).expect("a hit");
        assert_eq!(hit.submatches, [(0, 2)]);
    }

    #[test]
    fn the_pattern_travels_as_a_value_and_not_as_an_option() {
        let (program, args) = command("-w", true);
        assert_eq!(program, "rg");
        let at = args.iter().position(|arg| arg == "-e").expect("-e");
        assert_eq!(args[at + 1], "-w", "the query became an option of its own");
        assert_eq!(args[at + 2], "--");
        assert_eq!(args.last().expect("a path"), ".");
        assert!(!args.contains(&"--no-ignore".to_owned()));
    }

    #[test]
    fn ignore_is_turned_off_only_when_the_session_turns_it_off() {
        let (_, with_ignore) = command("x", true);
        assert!(!with_ignore.contains(&"--no-ignore".to_owned()));
        let (_, without_ignore) = command("x", false);
        assert!(without_ignore.contains(&"--no-ignore".to_owned()));
        // Every option still stands before the pattern and the paths.
        let dash_e = without_ignore
            .iter()
            .position(|arg| arg == "-e")
            .expect("-e");
        let no_ignore = without_ignore
            .iter()
            .position(|arg| arg == "--no-ignore")
            .expect("--no-ignore");
        assert!(no_ignore < dash_e);
    }

    #[test]
    fn the_offsets_of_a_hit_follow_the_line_as_it_is_drawn() {
        // The pane draws a tab as four spaces, so what is behind it moved.
        let ranges = display_ranges("\tabc", &[(1, 4)]);
        assert_eq!(ranges, [(4, 7)], "the tab took four columns");
        // A hit on a line of another script is where its bytes were.
        let ranges = display_ranges("日本語abc", &[(9, 12)]);
        assert_eq!(ranges, [(9, 12)], "three characters of three bytes");
        // An offset in the middle of a character is put back on one.
        let ranges = display_ranges("日本語", &[(1, 4)]);
        assert_eq!(ranges, [(0, 3)]);
        // And one past the line is not a range at all.
        assert!(display_ranges("ab", &[(7, 9)]).is_empty());
    }
}
