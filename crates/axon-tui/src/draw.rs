//! Drawing with ratatui on the full screen: the history pane (logo, finished
//! lines, the streaming reply), the activity row, a separator and the input.
//! While a change waits for approval, its diff is in the history and a y/n
//! prompt replaces the input.

use std::time::Duration;

use axon_protocol::{StopReason, ToolStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Stylize},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use crate::app::{Activity, App, Committed, Finished, Turn, View};
use crate::{diff, theme};

const AGENT: &str = "Agent ";
const INDENT: &str = "      ";
const PROMPT: &str = "> ";
const PROMPT_INDENT: &str = "  ";
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// Tool lines show at most this many chars of args or output.
const TOOL_PREVIEW: usize = 120;
/// A change shows at most this many rows of its diff.
const COMMITTED_DIFF_ROWS: usize = 20;
/// The input shows at most this many rows; longer input scrolls.
const INPUT_ROWS: usize = 5;
/// The y/n prompt, in parts of one colour each.
const APPROVAL_PROMPT: [(&str, Color, bool); 3] = [
    ("  [y] accept", theme::ACCEPT, true),
    ("  [n] reject", theme::REJECT, true),
    ("  · Esc cancels", theme::MUTED, false),
];

const LOGO: [&str; 6] = [
    " █████╗ ██╗  ██╗ ██████╗ ███╗   ██╗",
    "██╔══██╗╚██╗██╔╝██╔═══██╗████╗  ██║",
    "███████║ ╚███╔╝ ██║   ██║██╔██╗ ██║",
    "██╔══██║ ██╔██╗ ██║   ██║██║╚██╗██║",
    "██║  ██║██╔╝ ██╗╚██████╔╝██║ ╚████║",
    "╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝",
];

fn logo() -> Paragraph<'static> {
    Paragraph::new(
        LOGO.map(|line| Line::from(line).fg(theme::AGENT).bold())
            .to_vec(),
    )
}

/// A finished line for the history. Text wraps; tool lines are cut to the width.
fn committed(line: &Committed, width: u16) -> Paragraph<'static> {
    let (name, color, text) = match line {
        Committed::User(text) => ("You   ", theme::USER, text),
        Committed::Agent(text) => (AGENT, theme::AGENT, text),
        Committed::Error(text) => ("Error ", theme::ERROR, text),
        Committed::Cancelled => {
            return Paragraph::new(Line::from("  ✗   cancelled").fg(theme::ERROR));
        }
        Committed::Diff(change) => {
            let width = usize::from(width.max(1));
            return Paragraph::new(diff::lines(change, width, COMMITTED_DIFF_ROWS));
        }
        Committed::ToolResult {
            name,
            status: ToolStatus::Rejected,
            ..
        } => return tool_line("  ✗   ", theme::REJECTED, name, "rejected"),
        Committed::ToolCall { name, args } => {
            return tool_line("  →   ", theme::TOOL, name, &preview(args));
        }
        Committed::ToolResult {
            name,
            status,
            output,
        } => {
            let color = match status {
                ToolStatus::Completed => theme::TOOL,
                ToolStatus::Failed | ToolStatus::Rejected => theme::ERROR,
            };
            return tool_line("  ←   ", color, name, &preview(output));
        }
    };
    Paragraph::new(labelled(name, color, text)).wrap(Wrap { trim: false })
}

/// `text` with `name` before its first line and continuation lines indented.
fn labelled(name: &'static str, color: Color, text: &str) -> Text<'static> {
    text.split('\n')
        .enumerate()
        .map(|(i, part)| {
            let head = match i {
                0 => Span::from(name).fg(color).bold(),
                _ => Span::from(INDENT),
            };
            Line::from(vec![head, Span::from(part.replace('\t', "    "))])
        })
        .collect()
}

fn tool_line(arrow: &str, color: Color, name: &str, detail: &str) -> Paragraph<'static> {
    Paragraph::new(
        Line::from(vec![
            Span::from(format!("{arrow}{name}")).bold(),
            Span::from(format!("  {detail}")),
        ])
        .fg(color),
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

/// Draw the whole screen: the history, then the activity row, a separator
/// and the input. Returns the history pane's size, for scrolling.
pub(crate) fn screen(frame: &mut Frame, app: &App, history: &[Committed]) -> View {
    let area = frame.area();
    let width = usize::from(area.width.max(1));
    let (input, cursor) = match app.approval {
        Some(_) => (vec![approval_prompt()], None),
        None => {
            let (mut lines, (x, y)) = input(&app.input, width);
            let skip = lines.len().saturating_sub(INPUT_ROWS);
            lines.drain(..skip);
            (lines, Some((x, y - skip as u16)))
        }
    };
    let [history_area, _, activity_area, separator_area, input_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        // Always there, so the history doesn't move when a turn starts or ends.
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(input.len() as u16),
    ])
    .areas(area);

    let view = history_pane(frame, history_area, app, history);
    let status = match (&app.turn, &app.finished) {
        (Some(turn), _) => activity(turn),
        (None, Some(finished)) => finished_line(finished),
        (None, None) => Line::default(),
    };
    frame.render_widget(status, activity_area);
    frame.render_widget(
        Line::from("─".repeat(width)).fg(theme::MUTED),
        separator_area,
    );
    frame.render_widget(Paragraph::new(input), input_area);
    if let Some((x, y)) = cursor {
        frame.set_cursor_position(Position::new(input_area.x + x, input_area.y + y));
    }
    view
}

/// The logo at the top, then the history and the streaming reply. While they
/// are shorter than the pane they sit at its bottom, right above the input;
/// past that, `app.scroll` picks the rows shown.
fn history_pane(frame: &mut Frame, area: Rect, app: &App, history: &[Committed]) -> View {
    let mut items = vec![logo()];
    items.extend(history.iter().map(|line| committed(line, area.width)));
    let reply = app.streaming_text.trim_start();
    if !reply.is_empty() {
        items.push(Paragraph::new(labelled(AGENT, theme::AGENT, reply)).wrap(Wrap { trim: false }));
    }
    let heights: Vec<u32> = items
        .iter()
        .map(|item| u32::try_from(item.line_count(area.width)).unwrap_or(u32::MAX))
        .collect();
    let rows = heights.iter().fold(0u32, |sum, h| sum.saturating_add(*h));
    let view = View {
        height: area.height,
        rows,
    };
    let pane = u32::from(area.height);
    let gap = pane.saturating_sub(rows);
    let top = app.scroll.unwrap_or(view.bottom()).min(view.bottom());

    let mut start = 0u32;
    for (i, (item, height)) in items.into_iter().zip(heights).enumerate() {
        if i == 1 {
            start += gap;
        }
        let end = start.saturating_add(height);
        if end > top && start < top.saturating_add(pane) {
            let skip = u16::try_from(top.saturating_sub(start)).unwrap_or(u16::MAX);
            // Both are under the pane's height, so they fit a u16.
            let row = (start.saturating_sub(top)) as u16;
            let shown = (end.min(top + pane) - start.max(top)) as u16;
            let rect = Rect::new(area.x, area.y + row, area.width, shown);
            frame.render_widget(item.scroll((skip, 0)), rect);
        }
        start = end;
    }
    view
}

fn approval_prompt() -> Line<'static> {
    APPROVAL_PROMPT
        .into_iter()
        .map(|(text, color, bold)| match bold {
            true => Span::from(text).fg(color).bold(),
            false => Span::from(text).fg(color),
        })
        .collect()
}

/// The input as screen rows, wrapped at `width` chars, and where the cursor
/// goes: after the last char, or at the start of the next row when it's full.
fn input(input: &str, width: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut last = 0;
    for (i, line) in input.split('\n').enumerate() {
        let prefix = if i == 0 { PROMPT } else { PROMPT_INDENT };
        let chars: Vec<char> = format!("{prefix}{}", line.replace('\t', "    "))
            .chars()
            .collect();
        for (j, chunk) in chars.chunks(width).enumerate() {
            let row: String = chunk.iter().collect();
            last = chunk.len();
            rows.push(
                match row.strip_prefix(PROMPT).filter(|_| i == 0 && j == 0) {
                    Some(rest) => Line::from(vec![
                        Span::from(PROMPT).fg(theme::INPUT).bold(),
                        Span::from(rest.to_string()),
                    ]),
                    None => Line::from(row),
                },
            );
        }
    }
    if last == width {
        rows.push(Line::default());
        last = 0;
    }
    let cursor = (last as u16, rows.len() as u16 - 1);
    (rows, cursor)
}

/// The activity row while a turn runs, starting with the spinner.
fn activity(turn: &Turn) -> Line<'static> {
    let spinner = SPINNER[turn.frame % SPINNER.len()];
    let took = elapsed(turn.elapsed);
    let text = match &turn.activity {
        Activity::Thinking => format!("Thinking… {took} · Esc to cancel"),
        Activity::Running(tool) => format!("Running {tool}… {took} · Esc to cancel"),
        Activity::Approving => format!("Waiting for approval… {took}"),
        Activity::Cancelling => "Cancelling…".into(),
    };
    status(theme::AGENT, spinner, text)
}

/// The activity row once a turn has ended.
fn finished_line(finished: &Finished) -> Line<'static> {
    let (color, mark, word) = match finished.stop {
        StopReason::EndTurn => (theme::DONE, '✓', "Finished"),
        StopReason::Cancelled => (theme::ERROR, '✗', "Cancelled"),
        StopReason::MaxTurnRequests => (theme::ERROR, '✗', "Stopped"),
    };
    let at = finished.at.format("%H:%M");
    let took = elapsed(finished.took);
    status(color, mark, format!("{word} at {at} · {took}"))
}

/// A coloured mark, then muted text.
fn status(color: Color, mark: char, text: String) -> Line<'static> {
    Line::from(vec![
        Span::from(format!("{mark} ")).fg(color),
        Span::from(text).fg(theme::MUTED),
    ])
}

/// `2m 5s`, to the second.
fn elapsed(time: Duration) -> String {
    humantime::format_duration(Duration::from_secs(time.as_secs())).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    struct Screen {
        /// Every row as plain text, trailing spaces trimmed.
        rows: Vec<String>,
        buffer: Buffer,
        cursor: Position,
        view: View,
    }

    fn render(app: &App, history: &[Committed], width: u16, height: u16) -> Screen {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut view = View::default();
        terminal
            .draw(|frame| view = screen(frame, app, history))
            .unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let buffer = terminal.backend().buffer().clone();
        Screen {
            rows: rows(&buffer),
            buffer,
            cursor,
            view,
        }
    }

    fn text(paragraph: Paragraph, width: u16) -> Vec<String> {
        let height = paragraph.line_count(width) as u16;
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, width, height));
        ratatui::widgets::Widget::render(paragraph, buffer.area, &mut buffer);
        rows(&buffer)
    }

    /// Every row as plain text, trailing spaces trimmed.
    fn rows(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (area.top()..area.bottom())
            .map(|y| {
                let row: String = (area.left()..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                row.trim_end().to_string()
            })
            .collect()
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
            ..App::default()
        }
    }

    #[test]
    fn idle_screen_has_the_logo_on_top_and_the_input_at_the_bottom() {
        let screen = render(&app("", "hi"), &[], 40, 14);

        assert!(screen.rows[0].starts_with(" █████╗"));
        assert!(screen.rows[6..11].iter().all(String::is_empty));
        assert_eq!(screen.rows[12], "─".repeat(40));
        assert_eq!(screen.rows[13], "> hi");
        assert_eq!(screen.cursor, Position::new(4, 13));
        assert_eq!(
            screen.view,
            View {
                height: 10,
                rows: 6
            }
        );
    }

    #[test]
    fn turns_sit_right_above_the_input() {
        let history = [
            Committed::User("hi".into()),
            Committed::Agent("hello".into()),
        ];
        let screen = render(&app("one\ntwo", ""), &history, 40, 14);

        assert_eq!(
            screen.rows[6..10],
            ["You   hi", "Agent hello", "Agent one", "      two"]
        );
        assert_eq!(screen.rows[13], ">");
    }

    #[test]
    fn input_grows_to_five_rows_then_shows_its_tail() {
        let screen = render(&app("", &"y".repeat(60)), &[], 10, 20);

        // "> " + 60 chars is 7 rows; the last 5 show, under the separator.
        assert_eq!(screen.rows[14], "─".repeat(10));
        assert_eq!(screen.rows[15], "y".repeat(10));
        assert_eq!(screen.rows[19], "yy");
        assert_eq!(screen.cursor, Position::new(2, 19));
    }

    #[test]
    fn long_history_shows_its_tail_until_scrolled() {
        let reply: Vec<String> = (1..=30).map(|n| format!("line{n}")).collect();
        let mut app = app(&reply.join("\n"), "");

        let screen = render(&app, &[], 20, 10);
        assert_eq!(screen.rows[0], "      line25");
        assert_eq!(screen.rows[5], "      line30");
        assert_eq!(screen.rows[6], "", "an empty row above the activity row");
        assert_eq!(
            screen.view,
            View {
                height: 6,
                rows: 36
            }
        );

        app.scroll = Some(0);
        let screen = render(&app, &[], 20, 10);
        assert!(screen.rows[0].starts_with(" █████╗"));
        assert!(screen.rows[5].starts_with("╚═╝"));
    }

    #[test]
    fn scrolled_history_stays_put_while_the_reply_grows() {
        let reply: Vec<String> = (1..=30).map(|n| format!("line{n}")).collect();
        let mut app = app(&reply.join("\n"), "");
        app.scroll = Some(10);
        assert_eq!(render(&app, &[], 20, 10).rows[0], "      line5");

        app.streaming_text.push_str("\nline31");

        assert_eq!(render(&app, &[], 20, 10).rows[0], "      line5");
    }

    #[test]
    fn running_turn_adds_an_activity_row_above_the_separator() {
        let mut app = app("", "");
        app.turn = Some(busy_turn());
        let screen = render(&app, &[], 40, 14);

        assert_eq!(screen.rows[11], "⠋ Thinking… 0s · Esc to cancel");
        assert_eq!(screen.buffer[(0, 11)].fg, theme::AGENT);
        assert_eq!(screen.buffer[(2, 11)].fg, theme::MUTED);
    }

    #[test]
    fn ended_turn_shows_how_and_when_in_the_activity_row() {
        let mut app = app("", "");
        app.finished = Some(Finished {
            stop: StopReason::EndTurn,
            at: chrono::TimeZone::with_ymd_and_hms(&chrono::Local, 2026, 11, 17, 14, 32, 0)
                .unwrap(),
            took: Duration::from_millis(125_400),
        });
        let screen = render(&app, &[], 40, 14);

        assert_eq!(screen.rows[11], "✓ Finished at 14:32 · 2m 5s");
        assert_eq!(screen.buffer[(0, 11)].fg, theme::DONE);
    }

    #[test]
    fn elapsed_time_rolls_over_into_minutes_and_hours() {
        assert_eq!(elapsed(Duration::from_millis(900)), "0s");
        assert_eq!(elapsed(Duration::from_secs(59)), "59s");
        assert_eq!(elapsed(Duration::from_secs(3725)), "1h 2m 5s");
    }

    #[test]
    fn activity_row_shows_the_tool_and_cancelling() {
        let mut turn = busy_turn();
        turn.frame = 11;
        turn.activity = Activity::Running("list_dir".into());
        assert_eq!(
            activity(&turn).to_string(),
            "⠙ Running list_dir… 0s · Esc to cancel"
        );

        turn.activity = Activity::Cancelling;
        assert_eq!(activity(&turn).to_string(), "⠙ Cancelling…");
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
    fn tool_lines_are_cut_to_the_width() {
        let failed = Committed::ToolResult {
            name: "list_dir".into(),
            status: ToolStatus::Failed,
            output: "x".repeat(100),
        };

        let rows = text(committed(&failed, 30), 30);

        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("  ←   list_dir  xxx"), "{rows:?}");
    }

    #[test]
    fn committed_text_indents_continuation_lines_and_wraps() {
        let rows = text(committed(&Committed::User("a\nb".into()), 80), 80);
        assert_eq!(rows, ["You   a", "      b"]);

        let rows = text(committed(&Committed::Agent("word ".repeat(10)), 20), 20);
        assert_eq!(rows.len(), 3, "{rows:?}");
    }

    #[test]
    fn approval_shows_the_compact_diff_in_the_history_and_the_prompt_for_the_input() {
        let mut app = app("I'll write it.", "draft");
        app.turn = Some(busy_turn());
        let history = app.on_event(axon_protocol::Event::ApprovalRequested {
            id: 1,
            change: axon_protocol::Change::WriteFile {
                path: "a.txt".into(),
                old_text: None,
                new_text: (1..=30).map(|n| format!("{n}\n")).collect(),
            },
        });

        let screen = render(&app, &history, 40, 30);

        // The pane's 26 rows hold the end of the logo, the reply and 20 diff rows.
        assert_eq!(screen.rows[5], "Agent I'll write it.");
        assert_eq!(screen.rows[6], "  a.txt (new file)");
        assert_eq!(screen.rows[25], "  … 12 more lines");
        assert_eq!(screen.rows[26], "");
        assert!(screen.rows[27].contains("Waiting for approval"));
        assert_eq!(screen.rows[29], "  [y] accept  [n] reject  · Esc cancels");
        assert!(
            !screen.rows.iter().any(|r| r.contains("draft")),
            "the input is hidden"
        );
    }

    #[test]
    fn rejected_and_accepted_changes_commit_their_own_lines() {
        let rejected = Committed::ToolResult {
            name: "edit_file".into(),
            status: ToolStatus::Rejected,
            output: "the user rejected this change".into(),
        };
        assert_eq!(
            text(committed(&rejected, 80), 80),
            ["  ✗   edit_file  rejected"]
        );

        let accepted = Committed::Diff(axon_protocol::Change::WriteFile {
            path: "a.txt".into(),
            old_text: None,
            new_text: (1..=50).map(|n| format!("{n}\n")).collect(),
        });
        assert_eq!(
            text(committed(&accepted, 80), 80).len(),
            COMMITTED_DIFF_ROWS
        );
    }
}
