//! Inline drawing. Finished lines go into the terminal's scrollback; below
//! them, the live area (streaming reply, separator, input) is redrawn in place.

use std::io::{self, Write};

use crossterm::{
    cursor, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{Clear, ClearType},
};

use axon_protocol::ToolStatus;

use crate::app::{Activity, App, Committed, Turn};

const AGENT: &str = "Agent ";
const INDENT: &str = "      ";
const PROMPT: &str = "> ";
const PROMPT_INDENT: &str = "  ";
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// Tool lines show at most this many chars of args or output.
const TOOL_PREVIEW: usize = 120;

pub(crate) const LOGO: [&str; 6] = [
    " █████╗ ██╗  ██╗ ██████╗ ███╗   ██╗",
    "██╔══██╗╚██╗██╔╝██╔═══██╗████╗  ██║",
    "███████║ ╚███╔╝ ██║   ██║██╔██╗ ██║",
    "██╔══██║ ██╔██╗ ██║   ██║██║╚██╗██║",
    "██║  ██║██╔╝ ██╗╚██████╔╝██║ ╚████║",
    "╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝",
];

pub(crate) fn print_logo(out: &mut impl Write) -> io::Result<()> {
    for line in LOGO {
        label(out, line, Color::Cyan)?;
        queue!(out, Print("\r\n"))?;
    }
    out.flush()
}

/// Move to the top of the live area and clear everything below it.
/// Doesn't flush, so the clear and the repaint after it show as one frame.
pub(crate) fn clear_live(out: &mut impl Write, lines_to_top: u16) -> io::Result<()> {
    queue!(out, cursor::Hide, cursor::MoveToColumn(0))?;
    if lines_to_top > 0 {
        queue!(out, cursor::MoveUp(lines_to_top))?;
    }
    queue!(out, Clear(ClearType::FromCursorDown))
}

/// Print a finished line into the scrollback, at the top of a cleared live area.
pub(crate) fn commit(out: &mut impl Write, line: &Committed) -> io::Result<()> {
    let (name, color, text) = match line {
        Committed::User(text) => ("You   ", Color::Green, text),
        Committed::Agent(text) => (AGENT, Color::Cyan, text),
        Committed::Error(text) => ("Error ", Color::Red, text),
        Committed::Cancelled => {
            queue!(
                out,
                SetForegroundColor(Color::Red),
                Print("  ✗   cancelled"),
                ResetColor,
                Print("\r\n"),
            )?;
            return Ok(());
        }
        Committed::ToolCall { name, args } => {
            return tool_line(out, "  →   ", Color::Yellow, name, &preview(args));
        }
        Committed::ToolResult {
            name,
            status,
            output,
        } => {
            let color = match status {
                ToolStatus::Completed => Color::Magenta,
                ToolStatus::Failed => Color::Red,
            };
            return tool_line(out, "  ←   ", color, name, &preview(output));
        }
    };
    for (i, part) in text.split('\n').enumerate() {
        if i == 0 {
            label(out, name, color)?;
        } else {
            queue!(out, Print(INDENT))?;
        }
        queue!(out, Print(part), Print("\r\n"))?;
    }
    Ok(())
}

fn tool_line(
    out: &mut impl Write,
    arrow: &str,
    color: Color,
    name: &str,
    detail: &str,
) -> io::Result<()> {
    queue!(
        out,
        SetForegroundColor(color),
        Print(arrow),
        SetAttribute(Attribute::Bold),
        Print(name),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(format!("  {detail}")),
        ResetColor,
        Print("\r\n"),
    )
}

/// The first line of `text`, cut to `TOOL_PREVIEW` chars, with `…` if anything was left out.
fn preview(text: &str) -> String {
    let text = text.trim();
    let first = text.lines().next().unwrap_or("");
    let mut out: String = first.chars().take(TOOL_PREVIEW).collect();
    if out.len() < text.len() {
        out.push_str(" …");
    }
    out
}

/// Draw the live area at the cursor and leave the cursor at the end of the input.
/// Only the tail of the reply that fits on screen is shown; all of it is
/// committed when the turn ends.
/// Returns how many rows the cursor is below the top of the live area.
pub(crate) fn render_live(
    out: &mut impl Write,
    app: &App,
    width: u16,
    height: u16,
) -> io::Result<u16> {
    let width = usize::from(width.max(1));
    let input_rows: usize = app
        .input
        .split('\n')
        .map(|line| rows(PROMPT.len() + line.chars().count(), width))
        .sum();
    let activity_rows = usize::from(app.turn.is_some());
    let room = usize::from(height).saturating_sub(input_rows + activity_rows + 1);
    let reply = wrap(app.streaming_text.trim_start(), width);
    let skip = reply.len().saturating_sub(room);

    for (i, row) in reply.iter().enumerate().skip(skip) {
        match row.strip_prefix(AGENT).filter(|_| i == 0) {
            Some(rest) => {
                label(out, AGENT, Color::Cyan)?;
                queue!(out, Print(rest))?;
            }
            None => queue!(out, Print(row))?,
        }
        queue!(out, Print("\r\n"))?;
    }

    if let Some(turn) = &app.turn {
        let text: String = activity(turn).chars().take(width).collect();
        let (spinner, rest) = text.split_at(text.chars().next().map_or(0, char::len_utf8));
        queue!(
            out,
            SetForegroundColor(Color::Cyan),
            Print(spinner),
            SetForegroundColor(Color::DarkGrey),
            Print(rest),
            ResetColor,
            Print("\r\n"),
        )?;
    }

    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(width)),
        ResetColor,
        Print("\r\n"),
    )?;

    for (i, line) in app.input.split('\n').enumerate() {
        if i == 0 {
            label(out, PROMPT, Color::White)?;
        } else {
            queue!(out, Print("\r\n"), Print(PROMPT_INDENT))?;
        }
        queue!(out, Print(line))?;
    }
    queue!(out, cursor::Show)?;
    out.flush()?;

    Ok((reply.len() - skip + activity_rows + input_rows) as u16)
}

/// The activity row's text, starting with the spinner.
fn activity(turn: &Turn) -> String {
    let spinner = SPINNER[turn.frame % SPINNER.len()];
    let secs = turn.elapsed.as_secs();
    match &turn.activity {
        Activity::Thinking => format!("{spinner} Thinking… {secs}s · Esc to cancel"),
        Activity::Running(tool) => format!("{spinner} Running {tool}… {secs}s · Esc to cancel"),
        Activity::Cancelling => format!("{spinner} Cancelling…"),
    }
}

/// Split the reply into screen rows, prefixed like a committed agent line.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let prefix = if i == 0 { AGENT } else { INDENT };
        let chars: Vec<char> = format!("{prefix}{}", line.replace('\t', "    "))
            .chars()
            .collect();
        out.extend(chars.chunks(width).map(|row| row.iter().collect()));
    }
    out
}

/// Screen rows taken by a line of `len` chars.
fn rows(len: usize, width: usize) -> usize {
    len.div_ceil(width).max(1)
}

fn label(out: &mut impl Write, text: &str, color: Color) -> io::Result<()> {
    queue!(
        out,
        SetForegroundColor(color),
        SetAttribute(Attribute::Bold),
        Print(text),
        SetAttribute(Attribute::Reset),
        ResetColor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(app: &App, width: u16, height: u16) -> (String, u16) {
        let mut out = Vec::new();
        let lines_to_top = render_live(&mut out, app, width, height).unwrap();
        (String::from_utf8(out).unwrap(), lines_to_top)
    }

    fn busy_turn() -> Turn {
        let mut app = App {
            input: "x".into(),
            ..App::default()
        };
        app.on_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ));
        app.turn.unwrap()
    }

    fn app(streaming_text: &str, input: &str) -> App {
        App {
            input: input.into(),
            streaming_text: streaming_text.into(),
            turn: None,
        }
    }

    #[test]
    fn idle_live_area_is_a_separator_and_the_input() {
        let (out, lines_to_top) = render(&app("", "hi"), 20, 10);

        assert!(out.contains(&"─".repeat(20)));
        assert!(out.ends_with(&format!("hi{}", "\u{1b}[?25h")), "{out:?}");
        assert_eq!(lines_to_top, 1);
    }

    #[test]
    fn multiline_reply_and_input_count_every_row() {
        let (out, lines_to_top) = render(&app("one\ntwo", "a\nb"), 20, 10);

        assert!(out.contains("one\r\n      two\r\n"));
        assert!(out.contains("a\r\n  b"));
        assert_eq!(lines_to_top, 2 + 1 + 1);
    }

    #[test]
    fn long_lines_count_the_rows_they_wrap_to() {
        let (_, lines_to_top) = render(&app(&"x".repeat(30), &"y".repeat(15)), 10, 20);

        // "Agent " + 30 chars = 4 rows; "> " + 15 chars = 2 rows; cursor is on the last.
        assert_eq!(lines_to_top, 4 + 1 + 1);
    }

    #[test]
    fn reply_taller_than_the_screen_shows_only_its_tail() {
        let reply: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
        let (out, lines_to_top) = render(&app(&reply.join("\n"), ""), 20, 5);

        // 5 rows: 3 reply rows, the separator and the input.
        assert!(!out.contains("line7\r\n"));
        assert!(out.contains("line8\r\n") && out.contains("line10\r\n"));
        assert_eq!(lines_to_top, 4);
    }

    #[test]
    fn running_turn_adds_an_activity_row_above_the_separator() {
        let mut app = app("", "");
        app.turn = Some(busy_turn());
        let (out, lines_to_top) = render(&app, 40, 10);

        assert!(out.contains("⠋"));
        assert!(
            out.contains(" Thinking… 0s · Esc to cancel\u{1b}"),
            "{out:?}"
        );
        assert_eq!(lines_to_top, 2);
    }

    #[test]
    fn activity_row_shows_the_tool_and_cancelling() {
        let mut turn = busy_turn();
        turn.frame = 11;
        turn.activity = Activity::Running("list_dir".into());
        assert_eq!(activity(&turn), "⠙ Running list_dir… 0s · Esc to cancel");

        turn.activity = Activity::Cancelling;
        assert_eq!(activity(&turn), "⠙ Cancelling…");
    }

    #[test]
    fn preview_shows_the_first_line_and_marks_the_rest() {
        assert_eq!(preview("one line\n"), "one line");
        assert_eq!(preview("src/\nCargo.toml\n"), "src/ …");
        assert_eq!(
            preview(&"x".repeat(200)),
            format!("{} …", "x".repeat(TOOL_PREVIEW))
        );
    }

    #[test]
    fn tool_lines_show_name_and_preview() {
        let mut out = Vec::new();
        commit(
            &mut out,
            &Committed::ToolResult {
                name: "list_dir".into(),
                status: ToolStatus::Failed,
                output: "error: nope".into(),
            },
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();

        assert!(out.contains("←") && out.contains("list_dir") && out.contains("error: nope"));
        assert!(out.ends_with("\r\n"));
    }

    #[test]
    fn commit_indents_continuation_lines() {
        let mut out = Vec::new();
        commit(&mut out, &Committed::User("a\nb".into())).unwrap();
        let out = String::from_utf8(out).unwrap();

        assert!(out.contains("You   "));
        assert!(out.ends_with("a\r\n      b\r\n"), "{out:?}");
    }
}
