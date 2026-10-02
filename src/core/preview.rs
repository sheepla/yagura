//! Turning a file on disk into something the preview pane can draw.
//!
//! This is the part of the preview pipeline that touches the filesystem, so it
//! runs on a blocking worker (invariant I1). Everything it returns is already
//! sanitized: only strings that pass [`crate::core::text::sanitize`] may reach
//! the terminal (invariant I5).

use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::core::diff::{Diff, MAX_DIFF_LINES, Row};
use crate::core::highlight::Span;
use crate::core::text::{self, sanitize};

/// Bytes read from a file. Anything past this is dropped.
pub const MAX_BYTES: usize = 1024 * 1024;

/// Lines kept. Anything past this is dropped.
pub const MAX_LINES: usize = 2000;

/// A NUL in this many leading bytes means the file is binary.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Columns a line may occupy. A wider line is cut, because the diff engine
/// refuses to work on it and because drawing it would be pointless.
pub const MAX_LINE_COLS: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("cannot read file")]
    Io(#[from] io::Error),
}

/// The language a file is highlighted and labelled as.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Language {
    #[default]
    Plain,
    Rust,
    C,
    Cpp,
    CSharp,
    Go,
    Java,
    JavaScript,
    TypeScript,
    Python,
    Ruby,
    Shell,
    Json,
    Toml,
    Yaml,
    Markdown,
    Html,
    Css,
    Sql,
    Ini,
    Xml,
    Text,
}

impl Language {
    /// The name shown in the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Rust => "rust",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Go => "go",
            Self::Java => "java",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Ruby => "ruby",
            Self::Shell => "shell",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Css => "css",
            Self::Sql => "sql",
            Self::Ini => "ini",
            Self::Xml => "xml",
            Self::Text => "text",
        }
    }

    /// Detects the language from the file name, then from a shebang.
    ///
    /// The name is checked first because a `.md` file that happens to start
    /// with `#!` is markdown, not a script.
    pub fn detect(rel: &Path, head: &str) -> Self {
        if let Some(language) = from_extension(rel) {
            return language;
        }
        from_shebang(head).unwrap_or_default()
    }

    /// The label of this language, as a string the status bar can hold.
    pub fn name(self) -> &'static str {
        self.label()
    }
}

fn from_extension(rel: &Path) -> Option<Language> {
    let name = rel.file_name()?.to_string_lossy().to_lowercase();
    // A name like `Makefile` or `Dockerfile` has no extension at all, so the
    // extension is only looked at when there is a dot to split on.
    if let Some(ext) = name.rsplit_once('.').map(|(_, ext)| ext) {
        let language = match ext {
            "rs" => Language::Rust,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "cs" => Language::CSharp,
            "go" => Language::Go,
            "java" => Language::Java,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "tsx" | "mts" => Language::TypeScript,
            "py" | "pyi" => Language::Python,
            "rb" | "rake" | "gemspec" => Language::Ruby,
            "sh" | "bash" | "zsh" | "fish" => Language::Shell,
            "json" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "md" | "markdown" => Language::Markdown,
            "html" | "htm" => Language::Html,
            "css" => Language::Css,
            "sql" => Language::Sql,
            "ini" | "cfg" | "conf" => Language::Ini,
            "xml" => Language::Xml,
            "txt" | "log" => Language::Text,
            _ => return None,
        };
        return Some(language);
    }
    match name.as_str() {
        "makefile" | "gnumakefile" | "justfile" => Some(Language::Text),
        "dockerfile" | "containerfile" => Some(Language::Text),
        _ => None,
    }
}

fn from_shebang(head: &str) -> Option<Language> {
    let first = head.lines().next()?;
    let rest = first.strip_prefix("#!")?;
    // The interpreter is the last path segment: `#!/bin/bash` runs bash, and
    // `#!/usr/bin/env python3` names python3 after the env.
    let program = rest.split_whitespace().last()?.rsplit(['/', '\\']).next()?;
    let language = match program {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" => Language::Shell,
        "python" | "python3" | "pypy" => Language::Python,
        "ruby" | "jruby" | "rbx" => Language::Ruby,
        "node" | "bun" | "deno" => Language::JavaScript,
        _ => return None,
    };
    Some(language)
}

/// The kind of a file that is not shown as text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinaryKind {
    Image,
    Audio,
    Video,
    Archive,
    Executable,
    Document,
    Font,
    Other,
}

impl BinaryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Archive => "archive",
            Self::Executable => "executable",
            Self::Document => "document",
            Self::Font => "font",
            Self::Other => "binary",
        }
    }
}

impl fmt::Display for BinaryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

fn binary_kind(name: &str) -> BinaryKind {
    let ext = name
        .rsplit_once('.')
        .map_or("", |(_, ext)| ext)
        .to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "tiff" | "avif" | "svg" => {
            BinaryKind::Image
        }
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" => BinaryKind::Audio,
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "flv" => BinaryKind::Video,
        "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "tgz" => BinaryKind::Archive,
        "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "obj" | "a" | "lib" | "wasm" => {
            BinaryKind::Executable
        }
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "epub" => {
            BinaryKind::Document
        }
        "ttf" | "otf" | "woff" | "woff2" | "eot" => BinaryKind::Font,
        _ => BinaryKind::Other,
    }
}

/// A file, read and ready to draw.
///
/// The document is a plain list of already sanitized lines: what the terminal
/// would see is what is stored, so a reload can be swapped in without another
/// pass over the data and no later stage can reintroduce a control sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    /// Path relative to the root, the identity of the document.
    pub rel: PathBuf,
    /// File name, still an OS string so an invalid name is not a problem.
    pub name: OsString,
    /// Size on disk in bytes.
    pub size: u64,
    /// Detected language.
    pub language: Language,
    /// Lines, sanitized, without their line endings.
    pub lines: Vec<String>,
    /// The runs of meaning inside each line, by the same index as `lines`.
    ///
    /// Empty when the language has nothing to color, and never a different
    /// length from `lines` when it has (§9.3).
    pub spans: Vec<Vec<Span>>,
    /// The file was longer than [`MAX_BYTES`].
    pub truncated: bool,
    /// The file had more lines than [`MAX_LINES`].
    pub too_large: bool,
    /// Highlighting stopped at its line or time limit; the rest is plain.
    pub highlight_truncated: bool,
    /// The bytes were not valid UTF-8 and were replaced.
    pub invalid_utf8: bool,
    /// Set when the file is binary; only the kind and the size are shown then.
    pub binary: Option<BinaryKind>,
    /// Target of a symbolic link, shown instead of following it.
    pub link_target: Option<PathBuf>,
    /// The line diff against the baseline, once a diff mode drew one.
    ///
    /// [`Self::lines`] holds the text of every row while this holds the rows
    /// themselves, so a row and its kind can never drift apart. Empty when the
    /// reader asked for the plain content (§9.2).
    pub diff: Option<Diff>,
    /// Why a diff mode fell back to the plain content.
    pub diff_note: Option<DiffNote>,
}

impl Doc {
    /// True when the body has lines to draw.
    pub fn has_text(&self) -> bool {
        self.binary.is_none() && !self.lines.is_empty()
    }

    /// The word shown in the status bar: the language, or the binary kind.
    pub fn status_label(&self) -> &'static str {
        match self.binary {
            Some(kind) => kind.label(),
            None => self.language.label(),
        }
    }

    /// The header line: name, then everything the reader cannot see in the
    /// body itself.
    ///
    /// The flags are what tell a truncated file from a short one, so a line
    /// cut off is never mistaken for the end of the file.
    pub fn header(&self) -> String {
        let mut header = text::sanitize_os(&self.name);
        if let Some(target) = &self.link_target {
            header.push_str(" -> ");
            header.push_str(&text::sanitize_os(target.as_os_str()));
        }
        let mut notes: Vec<String> = Vec::new();
        if let Some(kind) = self.binary {
            notes.push(kind.label().to_owned());
        }
        notes.push(human_size(self.size));
        // A file past the byte limit and a file past the line limit are cut in
        // different places, so they are reported in different words.
        if self.too_large {
            notes.push("too large".to_owned());
        }
        if self.truncated {
            notes.push("truncated".to_owned());
        }
        if self.invalid_utf8 {
            notes.push("invalid UTF-8".to_owned());
        }
        if self.highlight_truncated {
            notes.push("highlight truncated".to_owned());
        }
        // The diff note says what the body itself cannot: why there is no
        // diff, or that it is only an approximation.
        if let Some(note) = self.diff_note
            && !notes.iter().any(|existing| existing == note.label())
        {
            notes.push(note.label().to_owned());
        }
        for note in notes {
            header.push_str("  ");
            header.push_str(&note);
        }
        header
    }
}

/// The baseline text a diff is computed against (§8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Baseline {
    /// The baseline could not be loaded; the body falls back to the content.
    Unavailable,
    /// The file is not in the baseline; every line counts as added.
    Absent,
    /// The baseline content as read from the repository.
    Bytes(Vec<u8>),
}

/// Why a diff mode has no diff to draw (§9.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffNote {
    /// No baseline could be resolved.
    NoBaseline,
    /// The file matches the baseline.
    NoChanges,
    /// The file or the diff passed a size limit.
    TooLarge,
    /// The diff ran past its time budget and is only approximate.
    Approximated,
}

impl DiffNote {
    /// The word added to the header.
    pub fn label(self) -> &'static str {
        match self {
            Self::NoBaseline => "no baseline",
            Self::NoChanges => "no changes",
            Self::TooLarge => "too large",
            Self::Approximated => "diff approximated",
        }
    }
}

/// Reads a file for the preview pane, as plain content.
pub fn load_doc(path: &Path, rel: &Path) -> Result<Doc, PreviewError> {
    load_doc_with(path, rel, None, false)
}

/// Reads a file for the preview pane, diffed against a baseline when the
/// reader asked for one.
///
/// A file that no longer exists still has its baseline: `want_diff` says
/// whether that baseline is drawn as a diff of every line it has, or shown
/// as plain content (§6).
pub fn load_doc_with(
    path: &Path,
    rel: &Path,
    baseline: Option<Baseline>,
    want_diff: bool,
) -> Result<Doc, PreviewError> {
    let name = path
        .file_name()
        .map_or_else(|| path.as_os_str().to_owned(), OsString::from);
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let doc = deleted_doc(rel, name, baseline, want_diff).ok_or(err)?;
            return Ok(doc);
        }
        Err(err) => return Err(err.into()),
    };
    let link_target = if metadata.file_type().is_symlink() {
        std::fs::read_link(path).ok()
    } else {
        None
    };
    let stat = std::fs::metadata(path)?;
    let size = stat.len();

    // One byte past the limit is enough to know that the limit was passed.
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_BYTES;
    if truncated {
        bytes.truncate(MAX_BYTES);
    }
    Ok(assemble(
        rel,
        name,
        size,
        link_target,
        bytes,
        truncated,
        baseline,
    ))
}

/// Builds the document of bytes read from a file that still exists.
fn assemble(
    rel: &Path,
    name: OsString,
    size: u64,
    link_target: Option<PathBuf>,
    bytes: Vec<u8>,
    truncated: bool,
    baseline: Option<Baseline>,
) -> Doc {
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        let kind = binary_kind(&name.to_string_lossy().to_lowercase());
        return Doc {
            rel: rel.to_path_buf(),
            name,
            size,
            language: Language::default(),
            lines: Vec::new(),
            spans: Vec::new(),
            truncated: false,
            too_large: false,
            highlight_truncated: false,
            invalid_utf8: false,
            binary: Some(kind),
            link_target,
            diff: None,
            diff_note: None,
        };
    }

    let invalid_utf8 = std::str::from_utf8(&bytes).is_err();
    let text = String::from_utf8_lossy(&bytes);
    let language = Language::detect(rel, &text);
    let (lines, too_large) = split_lines(&sanitize(&text));

    let mut doc = Doc {
        rel: rel.to_path_buf(),
        name,
        size,
        language,
        lines,
        spans: Vec::new(),
        truncated,
        too_large,
        highlight_truncated: false,
        invalid_utf8,
        binary: None,
        link_target,
        diff: None,
        diff_note: None,
    };
    // The pane draws colour, so a document is not ready until it has its
    // spans. This is the one place that runs on the worker anyway.
    crate::core::highlight::fill(&mut doc);
    if let Some(baseline) = baseline {
        apply_diff(&mut doc, baseline);
    }
    doc
}

/// The document of a file that no longer exists: the baseline stands in for
/// its content (§6).
///
/// `want_diff` folds every baseline line into a removed row, so a diff view
/// shows what the file lost while the content view reads like the file did.
fn deleted_doc(
    rel: &Path,
    name: OsString,
    baseline: Option<Baseline>,
    want_diff: bool,
) -> Option<Doc> {
    let Baseline::Bytes(content) = baseline? else {
        // Without the baseline's bytes there is nothing left to show.
        return None;
    };
    let size = content.len() as u64;
    let mut bytes = content;
    let truncated = bytes.len() > MAX_BYTES;
    if truncated {
        bytes.truncate(MAX_BYTES);
    }
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        let kind = binary_kind(&name.to_string_lossy().to_lowercase());
        return Some(Doc {
            rel: rel.to_path_buf(),
            name,
            size,
            language: Language::default(),
            lines: Vec::new(),
            spans: Vec::new(),
            truncated: false,
            too_large: false,
            highlight_truncated: false,
            invalid_utf8: false,
            binary: Some(kind),
            link_target: None,
            diff: None,
            diff_note: None,
        });
    }
    let invalid_utf8 = std::str::from_utf8(&bytes).is_err();
    let text = String::from_utf8_lossy(&bytes);
    let language = Language::detect(rel, &text);
    let (lines, too_large) = split_lines(&sanitize(&text));
    let mut doc = Doc {
        rel: rel.to_path_buf(),
        name,
        size,
        language,
        lines,
        spans: Vec::new(),
        truncated,
        too_large,
        highlight_truncated: false,
        invalid_utf8,
        binary: None,
        link_target: None,
        diff: None,
        diff_note: None,
    };
    crate::core::highlight::fill(&mut doc);
    if want_diff {
        if truncated || doc.too_large {
            // A baseline line is never drawn as it is, but past the limits
            // neither the engine nor the pane follows it (§9.2).
            doc.diff_note = Some(DiffNote::TooLarge);
        } else {
            let base_text = baseline_text(&bytes);
            fold_into(&mut doc, &base_text, "");
        }
    }
    Some(doc)
}

/// Computes the line diff and folds its rows into the document.
///
/// The document keeps showing the plain content whenever the diff cannot be
/// drawn, and a note in the header says why (§9.2).
fn apply_diff(doc: &mut Doc, baseline: Baseline) {
    if doc.binary.is_some() {
        return;
    }
    // The reader only ever sees a whole window of the file: diffing a cut
    // file against the repository would show changes that are not there.
    if doc.truncated || doc.too_large {
        doc.diff_note = Some(DiffNote::TooLarge);
        return;
    }
    let base_text = match baseline {
        Baseline::Unavailable => {
            doc.diff_note = Some(DiffNote::NoBaseline);
            return;
        }
        Baseline::Absent => String::new(),
        Baseline::Bytes(bytes) => baseline_text(&bytes),
    };
    let current = doc.lines.join("\n");
    fold_into(doc, &base_text, &current);
}

/// Folds the diff of `current` against `base` into the document.
fn fold_into(doc: &mut Doc, base_text: &str, current: &str) {
    let diff = match Diff::compute(base_text, current) {
        Ok(diff) if diff.rows.len() <= MAX_DIFF_LINES => diff,
        _ => {
            doc.diff_note = Some(DiffNote::TooLarge);
            return;
        }
    };
    if diff.stat.is_empty() {
        doc.diff_note = Some(DiffNote::NoChanges);
        return;
    }
    doc.lines = diff.rows.iter().map(row_text).collect();
    // A diff row is coloured by its kind alone, so the runs of meaning that
    // the highlighter found are dropped with the content lines.
    doc.spans.clear();
    doc.diff_note = diff.approximated.then_some(DiffNote::Approximated);
    doc.diff = Some(diff);
}

/// The baseline bytes as the diff engine and the pane both read them.
fn baseline_text(bytes: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(bytes);
    let sanitized = sanitize(&lossy);
    sanitized
        .lines()
        .map(cut_wide)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The words a row shows; a fold spells out what it hides.
fn row_text(row: &Row) -> String {
    match row {
        Row::Context(line) | Row::Added(line) | Row::Removed(line) => line.clone(),
        Row::Fold(count) => format!("\u{22ef} {count} lines"),
    }
}

/// Splits sanitized text into lines, dropping the tail past [`MAX_LINES`].
fn split_lines(sanitized: &str) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = sanitized.lines().map(cut_wide).collect();
    // A file that ends with a newline has no extra line, but `lines()` already
    // handles that, so nothing is invented here.
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        return (lines, true);
    }
    (lines, false)
}

/// Cuts a line that is wider than the diff engine is willing to look at.
fn cut_wide(line: &str) -> String {
    if text::display_width(line) <= MAX_LINE_COLS {
        return line.to_owned();
    }
    let mut out = String::new();
    let mut width = 0;
    for ch in line.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > MAX_LINE_COLS - 1 {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

/// A short size for the header, e.g. `1.2 MiB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, body: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn a_text_file_keeps_its_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\ntwo\nthree\n");
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        assert_eq!(doc.lines, vec!["one", "two", "three"]);
        assert_eq!(doc.language, Language::Text);
        assert!(!doc.truncated);
        assert!(!doc.too_large);
        assert!(!doc.invalid_utf8);
        assert_eq!(doc.binary, None);
    }

    #[test]
    fn carriage_returns_are_normalized_away() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\r\ntwo\r\n");
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        assert_eq!(doc.lines, vec!["one", "two"]);
    }

    #[test]
    fn a_lone_carriage_return_becomes_a_control_picture() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\rtwo\n");
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        assert_eq!(doc.lines, vec!["one\u{240d}two"]);
    }

    #[test]
    fn a_null_byte_in_the_head_makes_the_file_binary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.png", b"\x89PNG\r\n\x1a\n\0\0\0\0");
        let doc = load_doc(&path, Path::new("a.png")).expect("doc");
        assert_eq!(doc.binary, Some(BinaryKind::Image));
        assert!(!doc.has_text());
        assert!(doc.header().contains("image"), "{}", doc.header());
    }

    #[test]
    fn a_null_byte_past_the_sniff_window_is_still_text() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut body = vec![b'a'; BINARY_SNIFF_BYTES];
        body.push(0);
        let path = write(&dir, "a.txt", &body);
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        assert_eq!(doc.binary, None);
    }

    #[test]
    fn invalid_utf8_is_replaced_and_flagged() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"ok \xff\xfe end\n");
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        assert!(doc.invalid_utf8);
        assert!(doc.lines[0].contains('\u{fffd}'), "{:?}", doc.lines[0]);
    }

    #[test]
    fn a_file_past_the_byte_limit_is_truncated() {
        let dir = tempfile::tempdir().expect("temp dir");
        let body = vec![b'x'; MAX_BYTES + 10];
        let path = write(&dir, "big.txt", &body);
        let doc = load_doc(&path, Path::new("big.txt")).expect("doc");
        assert!(doc.truncated);
        assert_eq!(doc.size, (MAX_BYTES + 10) as u64);
        assert_eq!(text::display_width(&doc.lines[0]), MAX_LINE_COLS);
        assert!(doc.header().contains("truncated"));
    }

    #[test]
    fn a_file_past_the_line_limit_is_cut() {
        let dir = tempfile::tempdir().expect("temp dir");
        let body = (0..MAX_LINES + 5)
            .map(|i| format!("{i}\n"))
            .collect::<String>();
        let path = write(&dir, "long.txt", body.as_bytes());
        let doc = load_doc(&path, Path::new("long.txt")).expect("doc");
        assert_eq!(doc.lines.len(), MAX_LINES);
        assert!(doc.too_large);
    }

    #[test]
    fn a_wide_line_is_cut_at_the_column_limit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let body = format!("{}\n", "x".repeat(MAX_LINE_COLS + 50));
        let path = write(&dir, "wide.txt", body.as_bytes());
        let doc = load_doc(&path, Path::new("wide.txt")).expect("doc");
        assert_eq!(text::display_width(&doc.lines[0]), MAX_LINE_COLS);
    }

    #[test]
    fn control_sequences_never_reach_the_document() {
        let dir = tempfile::tempdir().expect("temp dir");
        let body = "\x1b[31mred\x1b[0m\n\u{202e}rtl\n";
        let path = write(&dir, "evil.txt", body.as_bytes());
        let doc = load_doc(&path, Path::new("evil.txt")).expect("doc");
        for line in &doc.lines {
            assert!(!line.contains('\u{1b}'), "{line:?}");
            assert!(!line.contains('\u{202e}'), "{line:?}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_is_shown_with_its_target() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(&dir, "real.txt", b"body\n");
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("link.txt"))
            .expect("symlink");
        let doc = load_doc(&dir.path().join("link.txt"), Path::new("link.txt")).expect("doc");
        let target = doc.link_target.expect("target");
        assert!(target.ends_with("real.txt"), "{target:?}");
        assert!(doc.header().contains("->"), "{}", doc.header());
        // The content still comes from the file the link points at.
        assert_eq!(doc.lines, vec!["body"]);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let err = load_doc(Path::new("/definitely/not/here"), Path::new("here")).expect_err("io");
        assert!(matches!(err, PreviewError::Io(_)));
        let message = match &err {
            PreviewError::Io(err) => text::os_error(err),
        };
        assert!(message.contains("not found"), "{message}");
    }

    #[test]
    fn the_language_comes_from_the_extension_first() {
        assert_eq!(Language::detect(Path::new("a.rs"), ""), Language::Rust);
        assert_eq!(
            Language::detect(Path::new("a.md"), "#!/bin/sh\n"),
            Language::Markdown
        );
    }

    #[test]
    fn the_language_falls_back_to_the_shebang() {
        assert_eq!(
            Language::detect(Path::new("run"), "#!/bin/bash\n"),
            Language::Shell
        );
        assert_eq!(
            Language::detect(Path::new("run"), "#!/usr/bin/env python3\n"),
            Language::Python
        );
        assert_eq!(
            Language::detect(Path::new("run"), "#!/usr/bin/env node\n"),
            Language::JavaScript
        );
    }

    #[test]
    fn a_file_without_a_clue_is_plain() {
        assert_eq!(
            Language::detect(Path::new("a.unknownext"), "hello"),
            Language::Plain
        );
        assert_eq!(Language::default().label(), "plain");
    }

    #[test]
    fn every_language_has_a_label() {
        let languages = [
            Language::Plain,
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::CSharp,
            Language::Go,
            Language::Java,
            Language::JavaScript,
            Language::TypeScript,
            Language::Python,
            Language::Ruby,
            Language::Shell,
            Language::Json,
            Language::Toml,
            Language::Yaml,
            Language::Markdown,
            Language::Html,
            Language::Css,
            Language::Sql,
            Language::Ini,
            Language::Xml,
            Language::Text,
        ];
        let mut labels: Vec<_> = languages.iter().map(|l| l.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), languages.len(), "labels must be distinct");
    }

    #[test]
    fn extension_matching_ignores_case() {
        assert_eq!(Language::detect(Path::new("A.RS"), ""), Language::Rust);
        assert_eq!(Language::detect(Path::new("A.Md"), ""), Language::Markdown);
    }

    #[test]
    fn a_makefile_is_text() {
        assert_eq!(Language::detect(Path::new("Makefile"), ""), Language::Text);
    }

    #[test]
    fn sizes_are_shown_in_units_a_person_reads() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn an_empty_file_has_no_lines_and_no_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "empty.txt", b"");
        let doc = load_doc(&path, Path::new("empty.txt")).expect("doc");
        assert!(doc.lines.is_empty());
        assert!(!doc.has_text());
        assert!(!doc.truncated);
    }

    #[test]
    fn a_plain_header_is_the_name_and_the_size() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"hello\n");
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        assert_eq!(doc.header(), "a.txt  6 B");
    }

    #[test]
    fn the_header_reports_why_the_body_stops_early() {
        let dir = tempfile::tempdir().expect("temp dir");
        // A file long enough to pass both limits is cut twice, and the header
        // has to say which limit was reached.
        let body = "x"
            .repeat(MAX_BYTES + 1)
            .as_bytes()
            .chunks(300)
            .map(|chunk| format!("{}\n", String::from_utf8_lossy(chunk)))
            .collect::<String>();
        assert!(
            body.len() > MAX_BYTES && body.lines().count() > MAX_LINES,
            "the fixture has to pass both limits"
        );
        let path = write(&dir, "both.txt", body.as_bytes());
        let doc = load_doc(&path, Path::new("both.txt")).expect("doc");
        assert!(doc.too_large && doc.truncated, "{doc:?}");
        let header = doc.header();
        assert!(header.contains("too large"), "{header}");
        assert!(header.contains("truncated"), "{header}");

        // Only the line limit, so "truncated" would be a lie.
        let lines_only = "z\n".repeat(MAX_LINES + 5);
        let path = write(&dir, "lines.txt", lines_only.as_bytes());
        let doc = load_doc(&path, Path::new("lines.txt")).expect("doc");
        assert!(doc.too_large && !doc.truncated, "{doc:?}");
        let header = doc.header();
        assert!(header.contains("too large"), "{header}");
        assert!(!header.contains("truncated"), "{header}");
    }

    #[test]
    fn the_header_says_when_the_bytes_were_not_text() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"ok \xff end\n");
        let doc = load_doc(&path, Path::new("a.txt")).expect("doc");
        let header = doc.header();
        assert!(header.contains("invalid UTF-8"), "{header}");
    }

    #[test]
    fn a_binary_header_leaves_out_the_text_flags() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.png", b"\x89PNG\r\n\x1a\n\0\0\0\0");
        let doc = load_doc(&path, Path::new("a.png")).expect("doc");
        let header = doc.header();
        assert!(header.starts_with("a.png  image"), "{header}");
        assert!(!header.contains("truncated"), "{header}");
        assert!(!header.contains("invalid"), "{header}");
    }

    #[test]
    fn a_program_comes_back_with_the_meaning_of_each_line() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "main.rs", b"fn main() {\n    // hi\n}\n");
        let doc = load_doc(&path, Path::new("main.rs")).expect("doc");
        assert_eq!(doc.language, Language::Rust);
        assert_eq!(doc.spans.len(), doc.lines.len());
        assert!(!doc.spans[0].is_empty(), "the first line came back plain");
        assert!(!doc.highlight_truncated);

        // A language with nothing to color keeps no runs at all, so the view
        // never has to tell the two cases apart by language.
        let path = write(&dir, "notes.txt", b"just words\n");
        let doc = load_doc(&path, Path::new("notes.txt")).expect("doc");
        assert!(doc.spans.is_empty());
    }

    #[test]
    fn the_header_says_when_the_colour_stopped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "main.rs", b"fn main() {}\n");
        let mut doc = load_doc(&path, Path::new("main.rs")).expect("doc");
        doc.highlight_truncated = true;
        let header = doc.header();
        assert!(header.contains("highlight truncated"), "{header}");
    }

    #[test]
    fn a_new_file_diffs_as_every_line_added() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\ntwo\n");
        let doc =
            load_doc_with(&path, Path::new("a.txt"), Some(Baseline::Absent), true).expect("doc");
        let diff = doc.diff.expect("a new file must have a diff");
        assert_eq!(diff.stat.added, 2);
        assert_eq!(diff.hunks, vec![0]);
        assert!(
            diff.rows.iter().all(|row| matches!(row, Row::Added(_))),
            "{:?}",
            diff.rows
        );
        assert_eq!(doc.lines, vec!["one", "two"]);
    }

    #[test]
    fn an_identical_file_reports_no_changes_in_the_header() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\ntwo\n");
        let baseline = Baseline::Bytes(b"one\ntwo\n".to_vec());
        let doc = load_doc_with(&path, Path::new("a.txt"), Some(baseline), true).expect("doc");
        assert!(doc.diff.is_none());
        assert_eq!(doc.diff_note, Some(DiffNote::NoChanges));
        assert_eq!(doc.lines, vec!["one", "two"]);
        assert!(doc.header().contains("no changes"), "{}", doc.header());
    }

    #[test]
    fn a_changed_file_is_folded_into_diff_rows() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\ntwo\nthree\n");
        let baseline = Baseline::Bytes(b"one\nTWO\nthree\n".to_vec());
        let doc = load_doc_with(&path, Path::new("a.txt"), Some(baseline), true).expect("doc");
        let diff = doc.diff.expect("diff");
        assert_eq!(diff.hunks, vec![1]);
        assert_eq!(doc.lines, vec!["one", "TWO", "two", "three"]);
        let kinds: Vec<&str> = diff
            .rows
            .iter()
            .map(|row| match row {
                Row::Context(_) => "context",
                Row::Added(_) => "added",
                Row::Removed(_) => "removed",
                Row::Fold(_) => "fold",
            })
            .collect();
        assert_eq!(kinds, vec!["context", "removed", "added", "context"]);
    }

    #[test]
    fn an_unavailable_baseline_falls_back_to_the_content() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"one\ntwo\n");
        let doc = load_doc_with(&path, Path::new("a.txt"), Some(Baseline::Unavailable), true)
            .expect("doc");
        assert!(doc.diff.is_none());
        assert_eq!(doc.diff_note, Some(DiffNote::NoBaseline));
        assert_eq!(doc.lines, vec!["one", "two"]);
        assert!(doc.header().contains("no baseline"), "{}", doc.header());
    }

    #[test]
    fn a_cut_file_is_not_diffed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let body = (0..MAX_LINES + 5)
            .map(|i| format!("{i}\n"))
            .collect::<String>();
        let path = write(&dir, "long.txt", body.as_bytes());
        let doc =
            load_doc_with(&path, Path::new("long.txt"), Some(Baseline::Absent), true).expect("doc");
        assert!(doc.diff.is_none());
        assert_eq!(doc.diff_note, Some(DiffNote::TooLarge));
        // The load limit and the skipped diff mean the same thing here, so
        // the header says it once.
        assert_eq!(
            doc.header().matches("too large").count(),
            1,
            "{}",
            doc.header()
        );
    }

    #[test]
    fn a_long_unchanged_run_folds_into_the_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let baseline = (1..=13)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let current = baseline.replace("line5", "changed");
        let path = write(&dir, "a.txt", current.as_bytes());
        let doc = load_doc_with(
            &path,
            Path::new("a.txt"),
            Some(Baseline::Bytes(baseline.into_bytes())),
            true,
        )
        .expect("doc");
        assert!(
            doc.lines.iter().any(|line| line == "\u{22ef} 2 lines"),
            "{:?}",
            doc.lines
        );
        assert!(doc.diff.is_some());
    }

    #[test]
    fn a_baseline_past_the_row_budget_is_not_diffed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"keep\n");
        let baseline = Baseline::Bytes("line\n".repeat(MAX_DIFF_LINES + 1).into_bytes());
        let doc = load_doc_with(&path, Path::new("a.txt"), Some(baseline), true).expect("doc");
        assert!(doc.diff.is_none());
        assert_eq!(doc.diff_note, Some(DiffNote::TooLarge));
    }

    #[test]
    fn a_control_sequence_in_the_baseline_never_reaches_the_rows() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "a.txt", b"red\n");
        let baseline = Baseline::Bytes(b"\x1b[31mred\x1b[0m\n".to_vec());
        let doc = load_doc_with(&path, Path::new("a.txt"), Some(baseline), true).expect("doc");
        assert!(doc.diff.is_some());
        for line in &doc.lines {
            assert!(!line.contains('\u{1b}'), "{line:?}");
        }
    }

    #[test]
    fn diff_rows_drop_the_syntax_runs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write(&dir, "main.rs", b"fn main() {}\n");
        let baseline = Baseline::Bytes(b"fn old() {}\n".to_vec());
        let doc = load_doc_with(&path, Path::new("main.rs"), Some(baseline), true).expect("doc");
        assert!(doc.diff.is_some());
        assert!(doc.spans.is_empty(), "colour comes from the row kind now");
    }

    #[test]
    fn a_deleted_file_is_read_from_the_baseline() {
        let dir = tempfile::tempdir().expect("temp dir");
        // The file itself was never written: only the baseline has it.
        let path = dir.path().join("gone.txt");
        let doc = load_doc_with(
            &path,
            Path::new("gone.txt"),
            Some(Baseline::Bytes(b"one\ntwo\n".to_vec())),
            false,
        )
        .expect("doc");
        assert_eq!(doc.lines, vec!["one", "two"]);
        assert_eq!(doc.size, 8);
        assert!(doc.diff.is_none(), "the content view draws no diff");
        assert!(doc.diff_note.is_none());
    }

    #[test]
    fn a_deleted_file_in_a_diff_mode_shows_every_line_removed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("gone.txt");
        let doc = load_doc_with(
            &path,
            Path::new("gone.txt"),
            Some(Baseline::Bytes(b"one\ntwo\n".to_vec())),
            true,
        )
        .expect("doc");
        let diff = doc.diff.expect("every baseline line was removed");
        assert_eq!(diff.stat.added, 0);
        assert_eq!(diff.stat.removed, 2);
        assert!(
            diff.rows
                .iter()
                .all(|row| matches!(row, crate::core::diff::Row::Removed(_))),
            "{:?}",
            diff.rows
        );
        assert_eq!(doc.lines, vec!["one", "two"]);
    }

    #[test]
    fn a_deleted_file_without_a_baseline_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("gone.txt");
        let err = load_doc_with(&path, Path::new("gone.txt"), None, true).expect_err("io");
        assert!(matches!(err, PreviewError::Io(_)));
    }
}
