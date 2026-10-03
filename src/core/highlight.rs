//! The meaning of each line, in the 16 terminal colors.
//!
//! `syntect` parses a line into scopes; a scope is turned into a [`Semantic`],
//! and the theme turns that into one of the ANSI 16 colors. No RGB is ever
//! produced, so the user's terminal palette stays in charge (§9.3).
//!
//! Parsing runs in the worker that reads the file, never in a reducer or the
//! view: it is CPU work and the frame must stay cheap (invariant I1).

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxSet};

use crate::core::preview::{Doc, Language, MAX_LINE_COLS};
use crate::core::text::display_width;

/// What a run of text means to a reader.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Semantic {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    /// The title line of a markdown heading (§9.4).
    Heading,
    /// `*emphasis*` and `**strong**` in markdown.
    Emphasis,
    /// The text of a quoted line, and the bar in front of it.
    Quote,
    /// One line inside a fenced markdown block.
    CodeBlock,
    /// `code` inside a markdown line.
    InlineCode,
    /// The text of a link, with the address left out.
    Link,
    /// Markdown kept as it was written: tables, images, raw HTML.
    Raw,
}

/// A run of text inside one line, as byte offsets into that line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// Byte offset the run starts at.
    pub start: usize,
    /// Byte offset the run ends at, exclusive.
    pub end: usize,
    /// What the run means.
    pub semantic: Semantic,
}

/// How far highlighting is allowed to reach into a document.
///
/// The parse has to walk from the first line or the state that carries across
/// lines is wrong for everything after a gap, so a run that runs out of budget
/// stops and leaves the rest plain.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Lines from the start of the document that are coloured.
    pub max_lines: usize,
    /// Time the whole document may take.
    pub max_duration: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_lines: 10_000,
            max_duration: Duration::from_millis(150),
        }
    }
}

/// The syntax definitions, loaded the first time a file is highlighted rather
/// than at startup, so the first frame is never held up by them.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);

/// One rule: a scope prefix and what a token carrying it means.
struct Rule {
    scope: Scope,
    semantic: Semantic,
}

/// Scope prefixes, from the most specific to the most general, so that
/// `storage.type` is a type while a bare `storage` is still a keyword.
static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    [
        ("comment", Semantic::Comment),
        ("string", Semantic::String),
        ("constant.numeric", Semantic::Number),
        ("entity.name.function", Semantic::Function),
        ("variable.function", Semantic::Function),
        ("support.function", Semantic::Function),
        ("entity.name.type", Semantic::Type),
        ("entity.name.class", Semantic::Type),
        ("entity.name.struct", Semantic::Type),
        ("entity.name.enum", Semantic::Type),
        ("entity.name.impl", Semantic::Type),
        ("support.type", Semantic::Type),
        // `storage.type` covers `let` and `i32` alike in the Rust syntax, so
        // a declaration word and the type it declares share this color.
        ("storage.type", Semantic::Type),
        ("keyword", Semantic::Keyword),
        ("storage.modifier", Semantic::Keyword),
        ("storage", Semantic::Keyword),
        ("constant.language", Semantic::Keyword),
    ]
    .into_iter()
    .map(|(scope, semantic)| Rule {
        scope: Scope::new(scope).expect("a scope prefix is well formed"),
        semantic,
    })
    .collect()
});

/// Fills the meaning of every line in `doc`.
///
/// Does nothing when there is nothing to colour: a binary, an empty file, and
/// a language with no rules.
pub fn fill(doc: &mut Doc) {
    fill_with(doc, Limits::default());
}

/// [`fill`] with the run allowed to stop early, which is what the tests use to
/// reach the limits without a document big enough to hit them by itself.
pub fn fill_with(doc: &mut Doc, limits: Limits) {
    doc.highlight_truncated = false;
    if doc.binary.is_some() {
        doc.spans.clear();
        return;
    }
    let (spans, truncated) = lines_of(doc.language, &doc.lines, limits);
    doc.spans = spans;
    doc.highlight_truncated = truncated;
}

/// The meaning of every one of `lines`, the way [`Doc::spans`] holds them.
///
/// The second half says whether the run stopped at a limit. An empty result
/// means there was nothing to colour: no lines, or a language with no rules.
pub fn lines_of(language: Language, lines: &[String], limits: Limits) -> (Vec<Vec<Span>>, bool) {
    if lines.is_empty() {
        return (Vec::new(), false);
    }
    let Some(token) = syntax_token(language) else {
        return (Vec::new(), false);
    };
    let syntaxes = &*SYNTAXES;
    let Some(syntax) = syntaxes.find_syntax_by_token(token) else {
        tracing::debug!(token, "no syntax definition for this language");
        return (Vec::new(), false);
    };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    // Parsing once compiles the syntax's rules, a cost paid once per process
    // rather than per document. It is warm-up, so the clock starts after it:
    // otherwise the first file the reader opens is the one that gets cut.
    let _ = ParseState::new(syntax).parse_line("", syntaxes);
    let deadline = Instant::now() + limits.max_duration;
    let mut out = Vec::with_capacity(lines.len());
    let mut truncated = false;
    for (index, line) in lines.iter().enumerate() {
        if index >= limits.max_lines || Instant::now() >= deadline {
            truncated = true;
            break;
        }
        // A line at the column limit is one that was cut, or a minified line.
        // It stays plain, but the parser still walks it: skipping a line would
        // leave the state wrong for every line after it.
        let plain = display_width(line) >= MAX_LINE_COLS;
        let spans = match state.parse_line(line, syntaxes) {
            Ok(ops) => walk(line, &ops, &mut stack, !plain),
            Err(err) => {
                tracing::debug!(?err, "the parser gave up on a line");
                Vec::new()
            }
        };
        out.push(spans);
    }
    // The limits can stop the run short, and a short `spans` would silently
    // hide colour from the view, so the untouched lines are filled in as
    // plain rather than left as a hole.
    out.resize_with(lines.len(), Vec::new);
    (out, truncated)
}

/// Walks one line's scope changes, recording where each meaning begins and
/// ends. Plain runs are left out; the view fills the gaps itself.
///
/// `record` is false for a line that must stay plain. The stack is walked
/// either way, because the scopes of one line carry into the next.
fn walk(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
    record: bool,
) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (pos, op) in ops {
        if *pos > start {
            if record {
                push_span(&mut spans, line, stack, start, *pos);
            }
            start = *pos;
        }
        if stack.apply(op).is_err() {
            // A definition that pops more than it pushed costs a color, never
            // the pane: the offsets stay valid either way.
            tracing::debug!("the scope stack drifted out of step");
        }
    }
    if record && start < line.len() {
        push_span(&mut spans, line, stack, start, line.len());
    }
    spans
}

/// Records `[start, end)` if it means something and its offsets are sound.
fn push_span(spans: &mut Vec<Span>, line: &str, stack: &ScopeStack, start: usize, end: usize) {
    // The offsets come from a regular expression; a broken one must not be
    // able to panic a slice in the view.
    if start >= end
        || !line.is_char_boundary(start)
        || !line.is_char_boundary(end)
        || end > line.len()
    {
        return;
    }
    let semantic = classify(&stack.scopes);
    if semantic == Semantic::Plain {
        return;
    }
    // The parser changes scope in the middle of a word more often than not.
    match spans.last_mut() {
        Some(last) if last.semantic == semantic && last.end == start => last.end = end,
        _ => spans.push(Span {
            start,
            end,
            semantic,
        }),
    }
}

/// The meaning of the innermost scope that says anything; plain when none of
/// the scopes on the stack is one that is given a color.
fn classify(stack: &[Scope]) -> Semantic {
    for scope in stack.iter().rev() {
        for rule in RULES.iter() {
            if rule.scope.is_prefix_of(*scope) {
                return rule.semantic;
            }
        }
    }
    Semantic::Plain
}

/// The syntax definition that covers a language, or nothing for the ones with
/// nothing to color: plain text has no rules, and Markdown is drawn
/// structurally instead of token by token (§9.4).
fn syntax_token(language: Language) -> Option<&'static str> {
    match language {
        Language::Plain | Language::Text | Language::Markdown => None,
        Language::Rust => Some("rs"),
        Language::C => Some("c"),
        Language::Cpp => Some("cpp"),
        Language::CSharp => Some("cs"),
        Language::Go => Some("go"),
        Language::Java => Some("java"),
        Language::JavaScript => Some("js"),
        // syntect ships no TypeScript grammar; the JavaScript one takes a
        // TypeScript file quite far, which is better than leaving it plain.
        Language::TypeScript => Some("js"),
        Language::Python => Some("py"),
        Language::Ruby => Some("rb"),
        Language::Shell => Some("sh"),
        Language::Json => Some("json"),
        // syntect ships no TOML grammar: the status bar still says `toml`,
        // and the body stays plain rather than asking for a token that does
        // not exist.
        Language::Toml => None,
        Language::Yaml => Some("yaml"),
        Language::Html => Some("html"),
        Language::Css => Some("css"),
        Language::Sql => Some("sql"),
        // No INI grammar either; the properties one reads the same
        // `key = value` lines an ini file is made of.
        Language::Ini => Some("properties"),
        Language::Xml => Some("xml"),
        Language::Makefile => Some("make"),
        Language::Php => Some("php"),
        Language::Lua => Some("lua"),
        Language::Perl => Some("perl"),
        Language::Scala => Some("scala"),
        Language::Haskell => Some("hs"),
        Language::Erlang => Some("erl"),
        Language::Groovy => Some("groovy"),
        Language::Clojure => Some("clj"),
        Language::Ocaml => Some("ml"),
        Language::R => Some("r"),
        Language::Matlab => Some("matlab"),
        Language::ObjectiveC => Some("m"),
        Language::Latex => Some("tex"),
        Language::Diff => Some("diff"),
        Language::ReStructuredText => Some("rst"),
        Language::Batch => Some("bat"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of `language` with one line per entry of `lines`.
    fn doc(language: Language, lines: &[String]) -> Doc {
        Doc {
            rel: "a".into(),
            name: "a".into(),
            size: 0,
            language,
            lines: lines.to_vec(),
            truncated: false,
            too_large: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
            formatted: false,
            spans: Vec::new(),
            highlight_truncated: false,
        }
    }

    /// [`fill`] with time to spare, so a test asserts on the spans rather
    /// than on how fast the machine happened to be that second.
    fn fill_all(doc: &mut Doc) {
        fill_with(
            doc,
            Limits {
                max_duration: Duration::from_secs(60),
                ..Limits::default()
            },
        );
    }

    /// The lines of `a.rs` that exercise every color.
    fn rust() -> Vec<String> {
        [
            "fn main() {",
            "    // say it",
            "    let name = \"yagura\";",
            "    let count = 42;",
            "    println!(\"{}\", name);",
            "}",
        ]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
    }

    /// The spans of one line as `(text, meaning)` pairs, which is easier to
    /// assert on than byte offsets.
    fn meaning(line: &str, spans: &[Span]) -> Vec<(String, Semantic)> {
        spans
            .iter()
            .map(|span| (line[span.start..span.end].to_owned(), span.semantic))
            .collect()
    }

    #[test]
    fn a_rust_file_gets_a_color_per_meaning() {
        let mut doc = doc(Language::Rust, &rust());
        fill_all(&mut doc);
        assert!(!doc.highlight_truncated);

        // `fn`, `let` and `i32` all carry `storage.type` in this syntax, so a
        // prefix cannot tell a declaration word from the type it declares and
        // they share one color; the name after `fn` is its own thing.
        assert_eq!(
            meaning(&doc.lines[0], &doc.spans[0]),
            vec![
                ("fn".to_owned(), Semantic::Type),
                ("main".to_owned(), Semantic::Function),
            ]
        );
        assert_eq!(
            meaning(&doc.lines[1], &doc.spans[1]),
            vec![("// say it".to_owned(), Semantic::Comment)]
        );
        // `name` means nothing on its own, so it is a gap between the runs
        // that do.
        assert_eq!(
            meaning(&doc.lines[2], &doc.spans[2]),
            vec![
                ("let".to_owned(), Semantic::Type),
                ("=".to_owned(), Semantic::Keyword),
                ("\"yagura\"".to_owned(), Semantic::String),
            ]
        );
        let count = meaning(&doc.lines[3], &doc.spans[3]);
        assert!(
            count.contains(&("let".to_owned(), Semantic::Type)),
            "{count:?}"
        );
        assert!(
            count.contains(&("=".to_owned(), Semantic::Keyword)),
            "{count:?}"
        );
        assert!(
            count.contains(&("42".to_owned(), Semantic::Number)),
            "{count:?}"
        );
    }

    #[test]
    fn spans_stay_inside_the_line_they_describe() {
        let mut doc = doc(Language::Rust, &rust());
        fill_all(&mut doc);
        for (index, spans) in doc.spans.iter().enumerate() {
            let line = &doc.lines[index];
            let mut previous_end = 0;
            for span in spans {
                assert!(span.start >= previous_end, "{span:?} after {previous_end}");
                assert!(span.end <= line.len(), "{span:?} past the line");
                assert!(span.start < span.end, "{span:?} is empty");
                previous_end = span.end;
            }
        }
    }

    #[test]
    fn a_language_without_rules_is_left_alone() {
        for language in [Language::Plain, Language::Text, Language::Markdown] {
            let mut doc = doc(language, &["# heading", "keyword = 1"].map(str::to_owned));
            fill(&mut doc);
            assert!(doc.spans.is_empty(), "{language:?} was coloured");
            assert!(!doc.highlight_truncated);
        }
    }

    /// A token with no grammar behind it silently drops every file of that
    /// language back to plain, so each one is checked against what syntect
    /// actually ships.
    #[test]
    fn every_language_finds_a_grammar_that_ships_with_syntect() {
        let all = [
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
            Language::Makefile,
            Language::Php,
            Language::Lua,
            Language::Perl,
            Language::Scala,
            Language::Haskell,
            Language::Erlang,
            Language::Groovy,
            Language::Clojure,
            Language::Ocaml,
            Language::R,
            Language::Matlab,
            Language::ObjectiveC,
            Language::Latex,
            Language::Diff,
            Language::ReStructuredText,
            Language::Batch,
            Language::Text,
        ];
        for language in all {
            match language {
                // Drawn structurally, not token by token.
                Language::Plain | Language::Text | Language::Markdown => {
                    assert!(
                        syntax_token(language).is_none(),
                        "{language:?} draws structurally but asked for a grammar"
                    );
                }
                // No grammar ships for it; the label still tells what it is.
                Language::Toml => assert!(
                    syntax_token(language).is_none(),
                    "{language:?} has no grammar but asked for one"
                ),
                _ => {
                    let token = syntax_token(language)
                        .unwrap_or_else(|| panic!("{language:?} forgot its grammar"));
                    assert!(
                        SYNTAXES.find_syntax_by_token(token).is_some(),
                        "{language:?} asks for {token:?}, which syntect does not ship"
                    );
                }
            }
        }
    }

    #[test]
    fn a_lua_file_gets_a_colour_per_meaning() {
        let mut doc = doc(
            Language::Lua,
            &["-- greet", "local n = 42"].map(str::to_owned),
        );
        fill_all(&mut doc);
        assert_eq!(
            meaning(&doc.lines[0], &doc.spans[0]),
            vec![("-- greet".to_owned(), Semantic::Comment)]
        );
        let local = meaning(&doc.lines[1], &doc.spans[1]);
        assert!(
            local.contains(&("42".to_owned(), Semantic::Number)),
            "{local:?}"
        );
        assert!(
            local
                .iter()
                .any(|(_, semantic)| *semantic == Semantic::Keyword || *semantic == Semantic::Type),
            "{local:?}"
        );
    }

    #[test]
    fn a_binary_has_nothing_to_colour() {
        let mut doc = doc(Language::Plain, &[]);
        doc.binary = Some(crate::core::preview::BinaryKind::Image);
        fill(&mut doc);
        assert!(doc.spans.is_empty());
    }

    #[test]
    fn a_line_at_the_column_limit_is_plain_but_the_next_one_is_not() {
        // A cut line: `MAX_LINE_COLS` wide once the ellipsis is added.
        let wide = format!("let s = \"{}\";", "x".repeat(MAX_LINE_COLS));
        let lines = vec![
            "fn f() {".to_owned(),
            format!("    {wide}"),
            "    let n = 42;".to_owned(),
        ];
        let mut doc = doc(Language::Rust, &lines);
        fill_all(&mut doc);
        assert!(
            doc.spans[1].is_empty(),
            "a wide line was coloured: {:?}",
            doc.spans[1]
        );
        // The parser still walked the wide line, so the line after it knows
        // where it is: `42` is a number and `=` is an operator.
        let after = meaning(&doc.lines[2], &doc.spans[2]);
        assert!(
            after.contains(&("=".to_owned(), Semantic::Keyword)),
            "{after:?}"
        );
        assert!(
            after.contains(&("42".to_owned(), Semantic::Number)),
            "{after:?}"
        );
    }

    #[test]
    fn the_line_limit_stops_the_run_and_says_so() {
        let mut doc = doc(Language::Rust, &rust());
        fill_with(
            &mut doc,
            Limits {
                max_lines: 2,
                max_duration: Duration::from_secs(60),
            },
        );
        assert!(doc.highlight_truncated);
        assert_eq!(doc.spans.len(), 6, "the rest of the lines still exist");
        assert!(!doc.spans[0].is_empty());
        assert!(!doc.spans[1].is_empty());
        assert!(doc.spans[2].is_empty(), "beyond the limit was coloured");
    }

    #[test]
    fn the_time_limit_stops_the_run_and_says_so() {
        let mut doc = doc(Language::Rust, &rust());
        fill_with(
            &mut doc,
            Limits {
                max_duration: Duration::ZERO,
                ..Limits::default()
            },
        );
        assert!(doc.highlight_truncated);
        assert!(doc.spans.iter().all(Vec::is_empty));
    }

    #[test]
    fn filling_twice_gives_the_same_spans() {
        let mut first = doc(Language::Rust, &rust());
        let mut second = doc(Language::Rust, &rust());
        fill_all(&mut first);
        fill_all(&mut second);
        assert_eq!(first.spans, second.spans);
    }

    #[test]
    fn a_parse_failure_leaves_the_line_plain_and_survives() {
        let lines = vec![
            "fn f() {".to_owned(),
            "\u{202e}rtl \u{1b}[31m".to_owned(),
            "    let n = 1;".to_owned(),
        ];
        let mut doc = doc(Language::Rust, &lines);
        fill_all(&mut doc);
        assert_eq!(doc.spans.len(), lines.len());
        let after = meaning(&doc.lines[2], &doc.spans[2]);
        assert!(
            after.contains(&("=".to_owned(), Semantic::Keyword)),
            "the line after the odd one was lost: {after:?}"
        );
        assert!(
            after.contains(&("1".to_owned(), Semantic::Number)),
            "the line after the odd one was lost: {after:?}"
        );
    }
}
