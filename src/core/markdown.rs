//! Markdown, drawn as structure instead of as source (§9.4).
//!
//! The layout runs where the file is read, on the worker (invariant I1), and
//! it keeps the line identity of the document: source line *i* stays at
//! position *i*, so the gutter's numbers, a grep hit and the scroll position
//! all still name the line they named before. Lines the syntax eats — a
//! fence, a setext underline — stay in the list as empty lines.
//!
//! The result holds the lines as they read *before* the pane is drawn: the
//! wrapping to the pane's width happens at draw time, so one layout serves
//! every width (§9.4).

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::core::highlight::{Semantic, Span};
use crate::core::preview::Doc;

/// The pieces of the syntax this layout understands; everything else in
/// CommonMark is left as it stands.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Lays `doc` out for reading, in place (§9.4).
///
/// Only the content view asks for this: a diff is computed against the file
/// as it stands on disk, not against the way it is drawn.
pub fn format(doc: &mut Doc) {
    let source = doc.lines.join("\n");
    let mut starts = Vec::with_capacity(doc.lines.len());
    let mut at = 0;
    for line in &doc.lines {
        starts.push(at);
        at += line.len() + 1;
    }
    let mut layout = Layout::new(&source, starts);
    for (event, range) in Parser::new_ext(&source, options()).into_offset_iter() {
        layout.apply(event, &range);
    }
    let (lines, spans) = layout.finish();
    doc.lines = lines;
    doc.spans = spans;
    doc.formatted = true;
}

/// One source line, waiting to be put back together.
struct OutLine {
    /// What stands in front of the line: the bar of a quote, the bullet of
    /// an item, the indent they nest with. `None` until the line has
    /// something written to it.
    prefix: Option<String>,
    text: String,
    spans: Vec<Span>,
}

/// The structure a line is nested in, remembered for its prefix.
enum Marker {
    Quote,
    /// An item of an ordered list keeps its number; of a bullet list it does
    /// not. `start` is the line the bullet itself stands on.
    Item {
        ordinal: Option<u64>,
        start: usize,
    },
}

/// The state of one open list, so that its items count up.
struct List {
    ordered: bool,
    next: u64,
}

struct Layout<'a> {
    /// The document the events are offsets into.
    source: &'a str,
    /// Byte offset each source line starts at.
    starts: Vec<usize>,
    lines: Vec<OutLine>,
    markers: Vec<Marker>,
    lists: Vec<List>,
    /// The meaning the text inside is given, innermost last.
    inline: Vec<Semantic>,
    code_blocks: usize,
    /// Above zero while a construct is drawn from its own source text, which
    /// swallows the events inside it.
    raw: usize,
}

impl<'a> Layout<'a> {
    fn new(source: &'a str, starts: Vec<usize>) -> Self {
        let lines = (0..starts.len())
            .map(|_| OutLine {
                prefix: None,
                text: String::new(),
                spans: Vec::new(),
            })
            .collect();
        Self {
            source,
            starts,
            lines,
            markers: Vec::new(),
            lists: Vec::new(),
            inline: Vec::new(),
            code_blocks: 0,
            raw: 0,
        }
    }

    fn apply(&mut self, event: Event<'_>, range: &Range<usize>) {
        if self.raw > 0 {
            // Only the end of the construct itself brings the layout back:
            // an end inside it belongs to something that was never opened.
            if matches!(
                event,
                Event::End(TagEnd::Table | TagEnd::HtmlBlock | TagEnd::Image)
            ) {
                self.raw -= 1;
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(end) => self.end(end),
            Event::Text(text) => {
                let semantic = self.semantic();
                self.route(range, &text, semantic);
            }
            Event::Code(text) => self.route(range, &text, Semantic::InlineCode),
            Event::Html(html) | Event::InlineHtml(html) => {
                self.route(range, &html, Semantic::Raw);
            }
            Event::FootnoteReference(label) => self.route(range, &label, Semantic::Raw),
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                self.route(range, mark, Semantic::Raw);
            }
            Event::InlineMath(expression) | Event::DisplayMath(expression) => {
                self.route(range, &expression, Semantic::Plain);
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: &Range<usize>) {
        match tag {
            Tag::BlockQuote(_) => {
                self.markers.push(Marker::Quote);
                self.inline.push(Semantic::Quote);
            }
            Tag::Item => {
                let ordinal = match self.lists.last_mut() {
                    Some(list) if list.ordered => {
                        let number = list.next;
                        list.next += 1;
                        Some(number)
                    }
                    _ => None,
                };
                let start = self.line_of(range.start);
                self.markers.push(Marker::Item { ordinal, start });
            }
            Tag::List(ordered) => self.lists.push(List {
                ordered: ordered.is_some(),
                next: ordered.unwrap_or(1),
            }),
            Tag::Heading { .. } => self.inline.push(Semantic::Heading),
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                self.inline.push(Semantic::Emphasis);
            }
            Tag::Link { .. } => self.inline.push(Semantic::Link),
            Tag::CodeBlock(_) => self.code_blocks += 1,
            Tag::Table(_) | Tag::HtmlBlock | Tag::Image { .. } => {
                self.route_raw(range);
                self.raw += 1;
            }
            _ => {}
        }
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::BlockQuote(_) => {
                self.markers.pop();
                self.inline.pop();
            }
            TagEnd::Item => {
                self.markers.pop();
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Heading(_)
            | TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link => {
                self.inline.pop();
            }
            TagEnd::CodeBlock => self.code_blocks = self.code_blocks.saturating_sub(1),
            _ => {}
        }
    }

    /// The meaning the text the parser is inside is given.
    fn semantic(&self) -> Semantic {
        if self.code_blocks > 0 {
            return Semantic::CodeBlock;
        }
        self.inline.last().copied().unwrap_or(Semantic::Plain)
    }

    /// Writes `text` to the line the range starts on, and to the lines after
    /// it when the text carries its own newlines (a code block's body).
    fn route(&mut self, range: &Range<usize>, text: &str, semantic: Semantic) {
        if text.is_empty() || self.lines.is_empty() {
            return;
        }
        let first = self.line_of(range.start);
        for (step, part) in text.split('\n').enumerate() {
            let Some(index) = first.checked_add(step) else {
                break;
            };
            if index >= self.lines.len() {
                break;
            }
            if part.is_empty() {
                continue;
            }
            if self.lines[index].prefix.is_none() {
                let prefix = self.prefix_for(index);
                self.lines[index].prefix = Some(prefix);
            }
            let line = &mut self.lines[index];
            let start = line.text.len();
            line.text.push_str(part);
            if semantic != Semantic::Plain {
                let end = line.text.len();
                match line.spans.last_mut() {
                    Some(last) if last.semantic == semantic && last.end == start => {
                        last.end = end;
                    }
                    _ => line.spans.push(Span {
                        start,
                        end,
                        semantic,
                    }),
                }
            }
        }
    }

    /// The construct as it stands in the file: a table or an image has
    /// nothing to draw but its own text, dim (§9.4).
    fn route_raw(&mut self, range: &Range<usize>) {
        let start = range.start.min(self.source.len());
        let end = range.end.max(start).min(self.source.len());
        let text = self.source[start..end].to_owned();
        self.route(range, &text, Semantic::Raw);
    }

    /// What stands in front of `line`: the bar of every quote it is inside,
    /// and the bullet of an item that begins on it (§9.4).
    fn prefix_for(&self, line: usize) -> String {
        let mut prefix = String::new();
        let mut depth = 0;
        for marker in &self.markers {
            match marker {
                Marker::Quote => prefix.push_str("│ "),
                Marker::Item { ordinal, start } => {
                    if *start == line {
                        prefix.push_str(&"  ".repeat(depth));
                        match ordinal {
                            Some(number) => {
                                prefix.push_str(&number.to_string());
                                prefix.push_str(". ");
                            }
                            None => prefix.push_str("• "),
                        }
                    }
                    depth += 1;
                }
            }
        }
        prefix
    }

    /// The lines as they read, with the prefixes put back in front of them.
    fn finish(self) -> (Vec<String>, Vec<Vec<Span>>) {
        let mut lines = Vec::with_capacity(self.lines.len());
        let mut spans = Vec::with_capacity(self.lines.len());
        for out in self.lines {
            let prefix = out.prefix.unwrap_or_default();
            if prefix.is_empty() {
                lines.push(out.text);
                spans.push(out.spans);
                continue;
            }
            let shift = prefix.len();
            let mut text = prefix;
            text.push_str(&out.text);
            let mut shifted = Vec::with_capacity(out.spans.len() + 1);
            shifted.push(Span {
                start: 0,
                end: shift,
                semantic: Semantic::Raw,
            });
            shifted.extend(out.spans.into_iter().map(|mut span| {
                span.start += shift;
                span.end += shift;
                span
            }));
            lines.push(text);
            spans.push(shifted);
        }
        (lines, spans)
    }

    /// The line a byte offset of the source belongs to, clamped to the last
    /// line for an offset at the very end of the document.
    fn line_of(&self, offset: usize) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        self.starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1)
            .min(self.lines.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document the layout can run over, as the loader would build it.
    fn doc(lines: &[&str]) -> Doc {
        Doc {
            rel: "README.md".into(),
            name: "README.md".into(),
            size: 0,
            language: crate::core::preview::Language::Markdown,
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            spans: Vec::new(),
            truncated: false,
            too_large: false,
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
            formatted: false,
        }
    }

    fn lay(lines: &[&str]) -> Doc {
        let mut doc = doc(lines);
        format(&mut doc);
        doc
    }

    /// The spans of one line as `(text, meaning)` pairs, which is easier to
    /// assert on than byte offsets.
    fn meaning(doc: &Doc, index: usize) -> Vec<(String, Semantic)> {
        let line = &doc.lines[index];
        doc.spans[index]
            .iter()
            .map(|span| (line[span.start..span.end].to_owned(), span.semantic))
            .collect()
    }

    #[test]
    fn a_heading_loses_its_hashes_and_keeps_its_place() {
        let doc = lay(&["# Title", "", "body"]);
        assert_eq!(doc.lines, vec!["Title", "", "body"]);
        assert_eq!(
            meaning(&doc, 0),
            vec![("Title".to_owned(), Semantic::Heading)]
        );
        assert!(doc.spans[1].is_empty());
        // Plain text is a gap between the runs that mean something, so the
        // body line has no run of its own.
        assert!(doc.spans[2].is_empty());
    }

    #[test]
    fn a_setext_heading_drops_its_underline() {
        let doc = lay(&["Title", "=====", "after"]);
        assert_eq!(doc.lines, vec!["Title", "", "after"]);
        assert_eq!(
            meaning(&doc, 0),
            vec![("Title".to_owned(), Semantic::Heading)]
        );
    }

    #[test]
    fn a_list_gets_bullets_and_numbers() {
        let doc = lay(&["- one", "- two", "", "1. first", "2. second"]);
        assert_eq!(
            doc.lines,
            vec!["• one", "• two", "", "1. first", "2. second"]
        );
    }

    #[test]
    fn a_nested_list_is_indented_under_its_parent() {
        let doc = lay(&["- outer", "  - inner"]);
        assert_eq!(doc.lines, vec!["• outer", "  • inner"]);
    }

    #[test]
    fn a_fenced_block_keeps_its_body_and_drops_the_fences() {
        let doc = lay(&["```rust", "fn main() {}", "```", "after"]);
        assert_eq!(doc.lines, vec!["", "fn main() {}", "", "after"]);
        assert_eq!(
            meaning(&doc, 1),
            vec![("fn main() {}".to_owned(), Semantic::CodeBlock)]
        );
    }

    #[test]
    fn a_quote_gets_a_bar_in_front_of_it() {
        let doc = lay(&["> quoted", "> more"]);
        assert_eq!(doc.lines, vec!["│ quoted", "│ more"]);
        assert_eq!(
            meaning(&doc, 0),
            vec![
                ("│ ".to_owned(), Semantic::Raw),
                ("quoted".to_owned(), Semantic::Quote),
            ]
        );
        assert_eq!(
            doc.spans[0][0],
            Span {
                start: 0,
                end: 4,
                semantic: Semantic::Raw
            }
        );
    }

    #[test]
    fn a_link_shows_its_text_and_leaves_the_address_out() {
        let doc = lay(&["see [site](https://example.com) now"]);
        assert_eq!(doc.lines, vec!["see site now"]);
        assert!(
            !doc.lines[0].contains("https"),
            "the address was drawn: {:?}",
            doc.lines[0]
        );
        assert_eq!(meaning(&doc, 0), vec![("site".to_owned(), Semantic::Link)]);
    }

    #[test]
    fn an_image_is_kept_as_written() {
        let doc = lay(&["![alt](pic.png)"]);
        assert_eq!(doc.lines, vec!["![alt](pic.png)"]);
        assert_eq!(
            meaning(&doc, 0),
            vec![("![alt](pic.png)".to_owned(), Semantic::Raw)]
        );
    }

    #[test]
    fn a_table_is_kept_as_written_line_by_line() {
        let source = ["| a | b |", "|---|---|", "| 1 | 2 |"];
        let doc = lay(&source);
        assert_eq!(doc.lines, source);
        for spans in &doc.spans {
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].semantic, Semantic::Raw);
        }
    }

    #[test]
    fn a_task_item_gets_a_checkbox() {
        let doc = lay(&["- [x] done", "- [ ] todo"]);
        assert_eq!(doc.lines, vec!["• [x] done", "• [ ] todo"]);
        assert_eq!(meaning(&doc, 0)[1], ("[x] ".to_owned(), Semantic::Raw));
    }

    #[test]
    fn emphasis_and_strength_are_both_drawn_as_emphasis() {
        let doc = lay(&["a *thin* and a **thick** word"]);
        assert_eq!(doc.lines, vec!["a thin and a thick word"]);
        let marks: Vec<_> = meaning(&doc, 0)
            .into_iter()
            .filter(|(_, semantic)| *semantic == Semantic::Emphasis)
            .collect();
        assert_eq!(
            marks,
            vec![
                ("thin".to_owned(), Semantic::Emphasis),
                ("thick".to_owned(), Semantic::Emphasis),
            ]
        );
    }

    /// Every source line keeps its place, so a jump to a line lands on it.
    #[test]
    fn the_layout_keeps_one_line_per_source_line() {
        let source = [
            "# Title",
            "",
            "A paragraph that",
            "spans two lines.",
            "",
            "- one",
            "- two",
            "",
            "```",
            "code",
            "```",
            "",
            "> quoted",
            "| a | b |",
            "|---|---|",
            "[link](https://example.com)",
            "![img](x.png)",
        ];
        let doc = lay(&source);
        assert_eq!(doc.lines.len(), source.len(), "{:#?}", doc.lines);
        assert_eq!(doc.spans.len(), source.len());
        assert!(doc.formatted);
    }

    #[test]
    fn the_spans_stay_inside_the_line_they_describe() {
        let doc = lay(&[
            "# Title",
            "- item with `code` and [a link](x)",
            "> quote **bold**",
            "```",
            "fn f() {}",
            "```",
        ]);
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
    fn an_empty_document_stays_empty() {
        let doc = lay(&[]);
        assert!(doc.lines.is_empty());
        assert!(doc.spans.is_empty());
    }

    #[test]
    fn layout_twice_gives_the_same_lines() {
        let source = ["# T", "- a", "> q", "text"];
        assert_eq!(lay(&source).lines, lay(&source).lines);
    }
}
