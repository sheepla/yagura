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

use crate::core::text::{self, sanitize};

/// Bytes read from a file. Anything past this is dropped.
pub const MAX_BYTES: usize = 1024 * 1024;

/// Lines kept. Anything past this is dropped.
pub const MAX_LINES: usize = 2000;

/// A NUL in this many leading bytes means the file is binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

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
    /// The file was longer than [`MAX_BYTES`].
    pub truncated: bool,
    /// The file had more lines than [`MAX_LINES`].
    pub too_large: bool,
    /// The bytes were not valid UTF-8 and were replaced.
    pub invalid_utf8: bool,
    /// Set when the file is binary; only the kind and the size are shown then.
    pub binary: Option<BinaryKind>,
    /// Target of a symbolic link, shown instead of following it.
    pub link_target: Option<PathBuf>,
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
        for note in notes {
            header.push_str("  ");
            header.push_str(&note);
        }
        header
    }
}

/// Reads a file for the preview pane.
pub fn load_doc(path: &Path, rel: &Path) -> Result<Doc, PreviewError> {
    let metadata = std::fs::symlink_metadata(path)?;
    let link_target = if metadata.file_type().is_symlink() {
        std::fs::read_link(path).ok()
    } else {
        None
    };
    let stat = std::fs::metadata(path)?;
    let name = path
        .file_name()
        .map_or_else(|| path.as_os_str().to_owned(), OsString::from);
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

    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        let kind = binary_kind(&name.to_string_lossy().to_lowercase());
        return Ok(Doc {
            rel: rel.to_path_buf(),
            name,
            size,
            language: Language::default(),
            lines: Vec::new(),
            truncated: false,
            too_large: false,
            invalid_utf8: false,
            binary: Some(kind),
            link_target,
        });
    }

    let invalid_utf8 = std::str::from_utf8(&bytes).is_err();
    let text = String::from_utf8_lossy(&bytes);
    let language = Language::detect(rel, &text);
    let (lines, too_large) = split_lines(&sanitize(&text));

    Ok(Doc {
        rel: rel.to_path_buf(),
        name,
        size,
        language,
        lines,
        truncated,
        too_large,
        invalid_utf8,
        binary: None,
        link_target,
    })
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
}
