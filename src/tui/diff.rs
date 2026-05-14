use std::io::{Stdout, Write};

use crossterm::{
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
};
use similar::{ChangeTag, TextDiff};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color as SynColor, ThemeSet},
    parsing::SyntaxSet,
};

const MAX_COMMITTED_LINES: usize = 20;

pub struct DiffRenderer {
    ss: SyntaxSet,
    ts: ThemeSet,
}

impl Default for DiffRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffRenderer {
    pub fn new() -> Self {
        Self {
            ss: two_face::syntax::extra_newlines(),
            ts: ThemeSet::load_defaults(),
        }
    }

    /// Full diff for live review; returns line count for cursor math.
    pub fn render_full(
        &self,
        stdout: &mut Stdout,
        path: &str,
        before: &str,
        after: &str,
    ) -> std::io::Result<u16> {
        self.render_diff(stdout, path, before, after, usize::MAX)
    }

    /// Compact diff for permanent terminal history.
    pub fn render_committed(
        &self,
        stdout: &mut Stdout,
        path: &str,
        before: &str,
        after: &str,
    ) -> std::io::Result<()> {
        self.render_diff(stdout, path, before, after, MAX_COMMITTED_LINES)?;
        Ok(())
    }

    fn render_diff(
        &self,
        stdout: &mut Stdout,
        path: &str,
        before: &str,
        after: &str,
        max_lines: usize,
    ) -> std::io::Result<u16> {
        let syntax = self
            .ss
            .find_syntax_for_file(path)
            .ok()
            .flatten()
            .or_else(|| self.ss.find_syntax_by_extension("txt"))
            .unwrap_or_else(|| self.ss.find_syntax_plain_text());

        let theme = &self.ts.themes["base16-ocean.dark"];

        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            SetAttribute(Attribute::Bold),
            Print(format!("  diff  {path}")),
            SetAttribute(Attribute::Reset),
            ResetColor,
            Print("\r\n"),
        )?;

        let diff = TextDiff::from_lines(before, after);
        let all_changes: Vec<_> = diff.iter_all_changes().collect();
        let total = all_changes.len();
        let mut old_line: u32 = 1;
        let mut new_line: u32 = 1;
        let mut rendered: usize = 0;

        for change in &all_changes {
            if rendered >= max_lines {
                let remaining = total - rendered;
                queue!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print(format!("  … {remaining} lines omitted")),
                    ResetColor,
                    Print("\r\n"),
                )?;
                rendered += 1;
                break;
            }

            let tag = change.tag();
            let line_text = change.value();

            let (bg, marker_color, marker, ln) = match tag {
                ChangeTag::Delete => (
                    Color::Rgb {
                        r: 80,
                        g: 25,
                        b: 25,
                    },
                    Color::Red,
                    '-',
                    format!("{:>4}       ", old_line),
                ),
                ChangeTag::Insert => (
                    Color::Rgb {
                        r: 25,
                        g: 70,
                        b: 25,
                    },
                    Color::Green,
                    '+',
                    format!("      {:>4} ", new_line),
                ),
                ChangeTag::Equal => (
                    Color::Reset,
                    Color::DarkGrey,
                    ' ',
                    format!("{:>4}  {:>4} ", old_line, new_line),
                ),
            };

            match tag {
                ChangeTag::Delete => old_line += 1,
                ChangeTag::Insert => new_line += 1,
                ChangeTag::Equal => {
                    old_line += 1;
                    new_line += 1;
                }
            }

            queue!(
                stdout,
                SetBackgroundColor(bg),
                SetForegroundColor(marker_color),
                Print(format!(" {marker} {ln}")),
            )?;

            for (color, text) in highlight_line(line_text, syntax, theme, &self.ss) {
                let t = text.trim_end_matches('\n');
                if t.is_empty() {
                    continue;
                }
                if let Some(c) = syn_to_crossterm(color) {
                    queue!(stdout, SetForegroundColor(c))?;
                }
                queue!(stdout, Print(t))?;
            }

            queue!(stdout, ResetColor, Print("\r\n"))?;
            rendered += 1;
        }

        stdout.flush()?;
        Ok((rendered + 1) as u16) // +1 for header
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn highlight_line(
    line: &str,
    syntax: &syntect::parsing::SyntaxReference,
    theme: &syntect::highlighting::Theme,
    ss: &SyntaxSet,
) -> Vec<(SynColor, String)> {
    let mut h = HighlightLines::new(syntax, theme);
    match h.highlight_line(line, ss) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| (style.foreground, text.to_owned()))
            .collect(),
        Err(_) => vec![(
            SynColor {
                r: 200,
                g: 200,
                b: 200,
                a: 255,
            },
            line.to_owned(),
        )],
    }
}

fn syn_to_crossterm(c: SynColor) -> Option<Color> {
    if c.a == 0 {
        None
    } else {
        Some(Color::Rgb {
            r: c.r,
            g: c.g,
            b: c.b,
        })
    }
}
