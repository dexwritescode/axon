//! Syntax-highlighted diffs of a proposed file change, one screen row per line.

use std::io::{self, Write};
use std::sync::LazyLock;

use axon_protocol::Change;
use crossterm::{
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
};
use similar::{ChangeTag, TextDiff};
use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;
/// Width of the `- 12   13 ` column before each line.
const GUTTER: usize = 12;

static SYNTAX: LazyLock<(SyntaxSet, Theme)> = LazyLock::new(|| {
    let mut themes = ThemeSet::load_defaults().themes;
    let theme = themes.remove("base16-ocean.dark").unwrap_or_default();
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
    },
    /// Rows cut off to fit.
    More(usize),
}

/// Draw `change` in at most `max_rows` rows (at least two) of `width`
/// columns; returns how many rows it took.
pub(crate) fn draw(
    out: &mut impl Write,
    change: &Change,
    width: usize,
    max_rows: usize,
) -> io::Result<usize> {
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
    for row in &rows {
        draw_row(out, row, width, syntax, syntaxes, theme)?;
        queue!(out, ResetColor, Print("\r\n"))?;
    }
    Ok(rows.len())
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
            for change in diff.iter_changes(op) {
                rows.push(Row::Line {
                    tag: change.tag(),
                    old: change.old_index().map(|n| n + 1),
                    new: change.new_index().map(|n| n + 1),
                    text: change
                        .value()
                        .trim_end_matches(['\n', '\r'])
                        .replace('\t', "    "),
                });
            }
        }
    }
    rows
}

fn draw_row(
    out: &mut impl Write,
    row: &Row,
    width: usize,
    syntax: &SyntaxReference,
    syntaxes: &SyntaxSet,
    theme: &Theme,
) -> io::Result<()> {
    let Row::Line {
        tag,
        old,
        new,
        text,
    } = row
    else {
        return match row {
            Row::Header(header) => {
                queue!(out, SetAttribute(Attribute::Bold))?;
                note(out, &format!("  {header}"), width)?;
                queue!(out, SetAttribute(Attribute::Reset))
            }
            Row::Gap => note(out, "  ⋯", width),
            Row::More(n) => note(out, &format!("  … {n} more lines"), width),
            Row::Line { .. } => unreachable!(),
        };
    };

    let (marker, color, background) = match tag {
        ChangeTag::Delete => (
            '-',
            Color::Red,
            Some(Color::Rgb {
                r: 80,
                g: 25,
                b: 25,
            }),
        ),
        ChangeTag::Insert => (
            '+',
            Color::Green,
            Some(Color::Rgb {
                r: 25,
                g: 70,
                b: 25,
            }),
        ),
        ChangeTag::Equal => (' ', Color::DarkGrey, None),
    };
    let number = |n: &Option<usize>| n.map_or(String::new(), |n| n.to_string());
    let gutter = format!("{marker} {:>4} {:>4} ", number(old), number(new));
    if let Some(background) = background {
        queue!(out, SetBackgroundColor(background))?;
    }
    queue!(
        out,
        SetForegroundColor(color),
        Print(gutter.chars().take(width).collect::<String>())
    )?;

    let mut room = width.saturating_sub(GUTTER);
    let mut highlighter = HighlightLines::new(syntax, theme);
    let line = format!("{text}\n");
    let pieces = highlighter
        .highlight_line(&line, syntaxes)
        .unwrap_or_default();
    for (style, piece) in pieces {
        let piece: String = piece.trim_end_matches('\n').chars().take(room).collect();
        room -= piece.chars().count();
        let c = style.foreground;
        queue!(
            out,
            SetForegroundColor(Color::Rgb {
                r: c.r,
                g: c.g,
                b: c.b
            }),
            Print(piece)
        )?;
    }
    Ok(())
}

fn note(out: &mut impl Write, text: &str, width: usize) -> io::Result<()> {
    let text: String = text.chars().take(width).collect();
    queue!(out, SetForegroundColor(Color::DarkGrey), Print(text))
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
        }
    }

    fn change(old_text: Option<&str>, new_text: &str) -> Change {
        Change::WriteFile {
            path: "src/lib.rs".into(),
            old_text: old_text.map(String::from),
            new_text: new_text.into(),
        }
    }

    /// The drawn text without escape sequences.
    fn plain(bytes: Vec<u8>) -> String {
        let text = String::from_utf8(bytes).unwrap();
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
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
        assert_eq!(rows[2], line(ChangeTag::Delete, Some(2), None, "2"));
        assert_eq!(rows[3], line(ChangeTag::Insert, None, Some(2), "two"));
        assert!(rows.contains(&Row::Gap));
        assert!(!rows.contains(&line(ChangeTag::Equal, Some(10), Some(10), "10")));
    }

    #[test]
    fn draw_fits_rows_and_width() {
        let new: String = (1..=50)
            .map(|n| format!("line {n} {}\n", "x".repeat(100)))
            .collect();
        let mut out = Vec::new();

        let used = draw(&mut out, &change(None, &new), 40, 10).unwrap();

        let text = plain(out);
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(used, 10);
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
}
