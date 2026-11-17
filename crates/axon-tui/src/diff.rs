//! Syntax-highlighted diffs of a proposed file change, one screen row per line.

use std::ops::Range;
use std::sync::LazyLock;

use crate::theme;
use axon_protocol::Change;
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span},
};
use similar::{ChangeTag, TextDiff};
use syntect::{
    easy::HighlightLines,
    highlighting::Theme,
    parsing::{SyntaxReference, SyntaxSet},
};

/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;
/// Width of the `- 12   13 ` column before each line.
const GUTTER: usize = 12;

static SYNTAX: LazyLock<(SyntaxSet, Theme)> = LazyLock::new(|| {
    let theme = two_face::theme::extra().get(theme::SYNTAX_THEME).clone();
    (two_face::syntax::extra_newlines(), theme)
});

#[derive(Debug, PartialEq)]
enum Row {
    Header(String),
    /// Unchanged lines left out between two changes.
    Gap,
    Line {
        tag: ChangeTag,
        old: Option<usize>,
        new: Option<usize>,
        text: String,
        /// Byte ranges of `text` that changed within the line.
        words: Vec<Range<usize>>,
    },
    /// Rows cut off to fit.
    More(usize),
}

/// `change` in at most `max_rows` lines (at least two) of `width` columns.
pub(crate) fn lines(change: &Change, width: usize, max_rows: usize) -> Vec<Line<'static>> {
    let Change::WriteFile {
        path,
        old_text,
        new_text,
    } = change;
    let mut rows = rows(path, old_text.as_deref(), new_text);
    let max_rows = max_rows.max(2);
    if rows.len() > max_rows {
        let hidden = rows.len() - (max_rows - 1);
        rows.truncate(max_rows - 1);
        rows.push(Row::More(hidden));
    }

    let (syntaxes, theme) = &*SYNTAX;
    let syntax = syntaxes
        .find_syntax_for_file(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    rows.iter()
        .map(|row| line(row, width, syntax, syntaxes, theme))
        .collect()
}

fn rows(path: &str, old_text: Option<&str>, new_text: &str) -> Vec<Row> {
    let header = match old_text {
        None => format!("{path} (new file)"),
        Some(_) => path.to_string(),
    };
    let diff = TextDiff::from_lines(old_text.unwrap_or(""), new_text);
    let mut rows = vec![Row::Header(header)];
    for (i, group) in diff.grouped_ops(CONTEXT).iter().enumerate() {
        if i > 0 {
            rows.push(Row::Gap);
        }
        for op in group {
            for change in diff.iter_inline_changes(op) {
                let mut text = String::new();
                let mut words = Vec::new();
                for (changed, part) in change.iter_strings_lossy() {
                    let start = text.len();
                    text.push_str(&part.trim_end_matches(['\n', '\r']).replace('\t', "    "));
                    if changed && text.len() > start {
                        words.push(start..text.len());
                    }
                }
                rows.push(Row::Line {
                    tag: change.tag(),
                    old: change.old_index().map(|n| n + 1),
                    new: change.new_index().map(|n| n + 1),
                    text,
                    words,
                });
            }
        }
    }
    rows
}

fn line(
    row: &Row,
    width: usize,
    syntax: &SyntaxReference,
    syntaxes: &SyntaxSet,
    theme: &Theme,
) -> Line<'static> {
    let (tag, old, new, text, words) = match row {
        Row::Header(header) => return note(format!("  {header}"), width).bold(),
        Row::Gap => return note("  ⋯".into(), width),
        Row::More(n) => return note(format!("  … {n} more lines"), width),
        Row::Line {
            tag,
            old,
            new,
            text,
            words,
        } => (tag, old, new, text, words),
    };

    let (marker, color, background) = match tag {
        ChangeTag::Delete => (
            '-',
            theme::DIFF_REMOVED,
            Some((theme::DIFF_REMOVED_BG, theme::DIFF_REMOVED_WORDS_BG)),
        ),
        ChangeTag::Insert => (
            '+',
            theme::DIFF_ADDED,
            Some((theme::DIFF_ADDED_BG, theme::DIFF_ADDED_WORDS_BG)),
        ),
        ChangeTag::Equal => (' ', theme::DIFF_CONTEXT, None),
    };
    let number = |n: &Option<usize>| n.map_or(String::new(), |n| n.to_string());
    let gutter = format!("{marker} {:>4} {:>4} ", number(old), number(new));
    let mut gutter_style = Style::new().fg(color);
    if let Some((line_bg, _)) = background {
        gutter_style = gutter_style.bg(line_bg);
    }
    let mut spans = vec![Span::styled(
        gutter.chars().take(width).collect::<String>(),
        gutter_style,
    )];

    let mut room = width.saturating_sub(GUTTER);
    let mut highlighter = HighlightLines::new(syntax, theme);
    let line = format!("{text}\n");
    let pieces = highlighter
        .highlight_line(&line, syntaxes)
        .unwrap_or_default();
    let mut at = 0;
    for (style, piece) in pieces {
        let c = style.foreground;
        let fg = Color::Rgb(c.r, c.g, c.b);
        for ch in piece.trim_end_matches('\n').chars() {
            if room == 0 {
                return Line::from(spans);
            }
            let mut style = Style::new().fg(fg);
            if let Some((line_bg, words_bg)) = background {
                let changed = words.iter().any(|w| w.contains(&at));
                style = style.bg(if changed { words_bg } else { line_bg });
            }
            match spans.last_mut() {
                Some(last) if last.style == style => last.content.to_mut().push(ch),
                _ => spans.push(Span::styled(ch.to_string(), style)),
            }
            at += ch.len_utf8();
            room -= 1;
        }
    }
    Line::from(spans)
}

fn note(text: String, width: usize) -> Line<'static> {
    Line::from(text.chars().take(width).collect::<String>()).fg(theme::MUTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(tag: ChangeTag, old: Option<usize>, new: Option<usize>, text: &str) -> Row {
        Row::Line {
            tag,
            old,
            new,
            text: text.into(),
            words: Vec::new(),
        }
    }

    /// The changed words of a line row.
    fn words(row: &Row) -> Vec<&str> {
        match row {
            Row::Line { text, words, .. } => words.iter().map(|w| &text[w.clone()]).collect(),
            _ => Vec::new(),
        }
    }

    fn change(old_text: Option<&str>, new_text: &str) -> Change {
        Change::WriteFile {
            path: "src/lib.rs".into(),
            old_text: old_text.map(String::from),
            new_text: new_text.into(),
        }
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn new_file_is_all_insertions() {
        assert_eq!(
            rows("a.rs", None, "one\ntwo\n"),
            [
                Row::Header("a.rs (new file)".into()),
                line(ChangeTag::Insert, None, Some(1), "one"),
                line(ChangeTag::Insert, None, Some(2), "two"),
            ]
        );
    }

    #[test]
    fn far_apart_changes_show_context_and_a_gap() {
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new: String = (1..=20)
            .map(|n| match n {
                2 => "two\n".to_string(),
                19 => "nineteen\n".to_string(),
                n => format!("{n}\n"),
            })
            .collect();

        let rows = rows("n.txt", Some(&old), &new);

        assert_eq!(rows[0], Row::Header("n.txt".into()));
        assert_eq!(rows[1], line(ChangeTag::Equal, Some(1), Some(1), "1"));
        assert_eq!(words(&rows[2]), ["2"]);
        assert_eq!(words(&rows[3]), ["two"]);
        assert!(rows.contains(&Row::Gap));
        assert!(!rows.contains(&line(ChangeTag::Equal, Some(10), Some(10), "10")));
    }

    #[test]
    fn changed_words_are_marked_within_a_line() {
        let rows = rows(
            "a.rs",
            Some("let name = \"world\";\n"),
            "let name = \"Dex\";\n",
        );

        assert_eq!(words(&rows[1]), ["\"world\";"]);
        assert_eq!(words(&rows[2]), ["\"Dex\";"]);
    }

    #[test]
    fn lines_fit_rows_and_width() {
        let new: String = (1..=50)
            .map(|n| format!("line {n} {}\n", "x".repeat(100)))
            .collect();

        let lines: Vec<String> = lines(&change(None, &new), 40, 10)
            .iter()
            .map(text)
            .collect();

        assert_eq!(lines.len(), 10);
        assert!(lines.iter().all(|l| l.chars().count() <= 40), "{lines:?}");
        assert_eq!(lines[0], "  src/lib.rs (new file)");
        assert_eq!(lines[9], "  … 42 more lines");
        assert!(
            lines[1].starts_with("+         1 line 1 x"),
            "{:?}",
            lines[1]
        );
    }

    #[test]
    fn changed_words_get_their_own_background() {
        let change = change(Some("let a = 1;\n"), "let a = 2;\n");

        let lines = lines(&change, 80, 10);

        let added = &lines[2];
        let bg_of = |s: &str| {
            added
                .spans
                .iter()
                .find(|span| span.content == s)
                .map(|span| span.style.bg)
        };
        assert_eq!(bg_of("2"), Some(Some(theme::DIFF_ADDED_WORDS_BG)));
        assert!(added.spans[1..].iter().all(|s| s.style.bg.is_some()));
    }
}
