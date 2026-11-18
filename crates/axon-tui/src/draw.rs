//! Drawing with ratatui on the full screen: the history pane (logo, finished
//! lines, the streaming reply), the activity row, a separator and the input.
//! While a change waits for approval, its whole diff or command is in the
//! history and a y/n prompt replaces the input; once answered, the diff is cut
//! to `COMMITTED_DIFF_ROWS`.

use std::time::Duration;

use axon_protocol::{Change, Exit, StopReason, ToolResult};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Stylize},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use crate::app::{Activity, App, Committed, Finished, Turn, View};
use crate::prompt::Prompt;
use crate::{diff, theme};

const AGENT: &str = "Agent ";
const INDENT: &str = "      ";
const PROMPT: &str = "> ";
const PROMPT_INDENT: &str = "  ";
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// Tool lines show at most this many chars of args or output.
const TOOL_PREVIEW: usize = 120;
/// An answered change shows at most this many rows of its diff.
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

/// A finished line for the history. Text wraps; tool lines are cut to the
/// width and a change's diff to `diff_rows`.
fn committed(line: &Committed, width: u16, diff_rows: usize) -> Paragraph<'static> {
    let (name, color, text) = match line {
        Committed::User(text) => ("You   ", theme::USER, text),
        Committed::Agent(text) => (AGENT, theme::AGENT, text),
        Committed::Error(text) => ("Error ", theme::ERROR, text),
        Committed::Cancelled => {
            return Paragraph::new(Line::from("  ✗   cancelled").fg(theme::ERROR));
        }
        Committed::Change(Change::WriteFile {
            path,
            old_text,
            new_text,
        }) => {
            let width = usize::from(width.max(1));
            let lines = diff::lines(path, old_text.as_deref(), new_text, width, diff_rows);
            return Paragraph::new(lines);
        }
        Committed::Change(Change::RunCommand { command }) => ("  $   ", theme::INPUT, command),
        Committed::ToolCall { name, args } => {
            return tool_line("  →   ", theme::TOOL, name, &preview(args));
        }
        Committed::ToolResult { name, result } => {
            return match result {
                ToolResult::Output(output) => {
                    tool_line("  ←   ", theme::TOOL, name, &preview(output))
                }
                ToolResult::Failed(error) => {
                    tool_line("  ←   ", theme::ERROR, name, &preview(error))
                }
                ToolResult::Rejected => tool_line("  ✗   ", theme::REJECTED, name, "rejected"),
                ToolResult::Command { head, exit, .. } => command_result(name, head, *exit),
            };
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

/// A command's result: how it ended, unless it exited 0, then the start of
/// what it printed.
fn command_result(name: &str, head: &str, exit: Exit) -> Paragraph<'static> {
    let ending = match exit {
        Exit::Code(0) => return tool_line("  ←   ", theme::TOOL, name, &preview(head)),
        Exit::Code(code) => format!("exit {code}"),
        Exit::Killed => "killed".into(),
        Exit::TimedOut { secs } => format!("timed out after {secs} s"),
        Exit::Cancelled => "cancelled".into(),
    };
    let detail = match preview(head).as_str() {
        "" => ending,
        printed => format!("{ending} · {printed}"),
    };
    tool_line("  ←   ", theme::ERROR, name, &detail)
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
/// and the input. Returns the sizes keys need.
pub(crate) fn screen(frame: &mut Frame, app: &App, history: &[Committed]) -> View {
    let area = frame.area();
    let width = usize::from(area.width.max(1));
    // One column past the text is left for the cursor.
    let input_width = width.saturating_sub(PROMPT.len() + 1).max(1);
    let mut input_top = app.view.input_top;
    let (input, cursor) = match app.approval {
        Some(_) => (vec![approval_prompt()], None),
        None => {
            let (lines, cursor, top) = input(&app.input, input_width, input_top);
            input_top = top;
            (lines, Some(cursor))
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

    let view = View {
        input_width,
        input_top,
        ..history_pane(frame, history_area, app, history)
    };
    let status = match (&app.turn, &app.finished) {
        (Some(turn), _) => activity(turn),
        (None, Some(finished)) => finished_line(finished, app.context_limit),
        (None, None) => Line::default(),
    };
    frame.render_widget(status, activity_area);
    frame.render_widget(
        Line::from("─".repeat(width)).fg(theme::MUTED),
        separator_area,
    );
    frame.render_widget(Paragraph::new(input), input_area);
    if let Some(cursor) = cursor {
        frame.set_cursor_position(Position::new(
            input_area.x + cursor.x,
            input_area.y + cursor.y,
        ));
    }
    view
}

/// The logo at the top, then the history and the streaming reply. While they
/// are shorter than the pane they sit at its bottom, right above the input;
/// past that, `app.scroll` picks the rows shown.
fn history_pane(frame: &mut Frame, area: Rect, app: &App, history: &[Committed]) -> View {
    let mut items = vec![logo()];
    // The change waiting for approval is the last line; show all of it.
    let pending = app.approval.map(|_| history.len().saturating_sub(1));
    items.extend(history.iter().enumerate().map(|(i, line)| {
        let diff_rows = if Some(i) == pending {
            usize::MAX
        } else {
            COMMITTED_DIFF_ROWS
        };
        committed(line, area.width, diff_rows)
    }));
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
        ..View::default()
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

/// The input's rows, `> ` before the first and `  ` before the rest, at
/// most `INPUT_ROWS` of them from `top`, moved only as far as needed to
/// show the cursor; where the cursor goes; and the first row shown. The
/// text wraps at `width` columns.
fn input(prompt: &Prompt, width: usize, top: usize) -> (Vec<Line<'static>>, Position, usize) {
    let rows = prompt.rows(width);
    let (col, row) = prompt.cursor_at(width);
    let skip = top
        .min(rows.len().saturating_sub(INPUT_ROWS))
        .clamp((row + 1).saturating_sub(INPUT_ROWS), row);
    let lines = rows
        .into_iter()
        .enumerate()
        .skip(skip)
        .take(INPUT_ROWS)
        .map(|(i, range)| {
            let head = match i {
                0 => Span::from(PROMPT).fg(theme::INPUT).bold(),
                _ => Span::from(PROMPT_INDENT),
            };
            Line::from(vec![head, Span::from(prompt.text()[range].to_string())])
        })
        .collect();
    let cursor = Position::new((PROMPT.len() + col) as u16, (row - skip) as u16);
    (lines, cursor, skip)
}

/// The activity row while a turn runs, starting with the spinner.
fn activity(turn: &Turn) -> Line<'static> {
    let spinner = SPINNER[turn.frame % SPINNER.len()];
    let took = elapsed(turn.elapsed);
    let text = match &turn.activity {
        Activity::Thinking => format!("Thinking… {took} · Esc to cancel"),
        Activity::Running(tool) => format!("Running {tool}… {took} · Esc to cancel"),
        Activity::Approving => format!("Waiting for approval… {took}"),
        Activity::Cooling { until } if turn.elapsed < *until => {
            let left = elapsed(*until - turn.elapsed);
            format!("Cooling down… {left} · Esc to cancel")
        }
        Activity::Cooling { .. } => format!("Thinking… {took} · Esc to cancel"),
        Activity::Cancelling => "Cancelling…".into(),
    };
    status(theme::AGENT, spinner, text)
}

/// The activity row once a turn has ended.
fn finished_line(finished: &Finished, context_limit: Option<u64>) -> Line<'static> {
    let (color, mark, word) = match finished.stop {
        StopReason::EndTurn => (theme::DONE, '✓', "Finished"),
        StopReason::Cancelled => (theme::ERROR, '✗', "Cancelled"),
        StopReason::MaxTurnRequests => (theme::ERROR, '✗', "Stopped"),
    };
    let at = finished.at.format("%H:%M");
    let took = elapsed(finished.took);
    let context = match context_limit {
        Some(tokens) if tokens >= 1000 => format!(" · {}k context", tokens / 1000),
        Some(tokens) => format!(" · {tokens} context"),
        None => String::new(),
    };
    status(color, mark, format!("{word} at {at} · {took}{context}"))
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
                rows: 6,
                input_width: 37,
                input_top: 0,
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

        // 60 chars at 7 a row is 9 rows; the last 5 show, under the separator.
        assert_eq!(screen.rows[14], "─".repeat(10));
        assert_eq!(screen.rows[15], format!("  {}", "y".repeat(7)));
        assert_eq!(screen.rows[19], "  yyyy");
        assert_eq!(screen.cursor, Position::new(6, 19));
    }

    #[test]
    fn input_rows_move_only_to_keep_the_cursor_in_view() {
        let mut app = app("", "one\ntwo\nthree\nfour\nfive\nsix\nseven");
        // The first input row shown and the cursor's screen row.
        let redraw = |app: &mut App, ups: usize, downs: usize| {
            (0..ups).for_each(|_| app.input.up(20));
            (0..downs).for_each(|_| app.input.down(20));
            let screen = render(app, &[], 30, 20);
            app.view = screen.view;
            (screen.rows[15].clone(), screen.cursor.y)
        };

        assert_eq!(redraw(&mut app, 0, 0), ("  three".into(), 19));
        assert_eq!(redraw(&mut app, 3, 0), ("  three".into(), 16));
        assert_eq!(redraw(&mut app, 2, 0), ("  two".into(), 15));
        assert_eq!(redraw(&mut app, 0, 1), ("  two".into(), 16));
        assert_eq!(redraw(&mut app, 2, 0), ("> one".into(), 15));
    }

    #[test]
    fn input_wraps_words_and_places_the_cursor_mid_text() {
        let mut app = app("", "hello wide 世界");
        app.input.word_left();
        let screen = render(&app, &[], 13, 20);

        // Each wide char takes two cells; the second reads as a space here.
        assert_eq!(screen.rows[18..20], ["> hello wide", "  世 界"]);
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
                rows: 36,
                input_width: 17,
                input_top: 0,
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
    fn the_finished_line_shows_the_context_limit_when_known() {
        let mut app = app("", "");
        app.context_limit = Some(262_144);
        app.finished = Some(Finished {
            stop: StopReason::EndTurn,
            at: chrono::TimeZone::with_ymd_and_hms(&chrono::Local, 2026, 11, 17, 14, 32, 0)
                .unwrap(),
            took: Duration::from_millis(125_400),
        });
        let screen = render(&app, &[], 60, 14);

        assert_eq!(
            screen.rows[11],
            "✓ Finished at 14:32 · 2m 5s · 262k context"
        );
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
    fn activity_row_counts_down_a_pause_then_thinks() {
        let mut turn = busy_turn();
        turn.activity = Activity::Cooling {
            until: Duration::from_secs(30),
        };
        turn.elapsed = Duration::from_secs(5);
        assert_eq!(
            activity(&turn).to_string(),
            "⠋ Cooling down… 25s · Esc to cancel"
        );

        turn.elapsed = Duration::from_secs(30);
        assert_eq!(
            activity(&turn).to_string(),
            "⠋ Thinking… 30s · Esc to cancel"
        );
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
            result: ToolResult::Failed("x".repeat(100)),
        };

        let rows = text(committed(&failed, 30, COMMITTED_DIFF_ROWS), 30);

        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("  ←   list_dir  xxx"), "{rows:?}");
    }

    #[test]
    fn committed_text_indents_continuation_lines_and_wraps() {
        let rows = text(
            committed(&Committed::User("a\nb".into()), 80, COMMITTED_DIFF_ROWS),
            80,
        );
        assert_eq!(rows, ["You   a", "      b"]);

        let rows = text(
            committed(
                &Committed::Agent("word ".repeat(10)),
                20,
                COMMITTED_DIFF_ROWS,
            ),
            20,
        );
        assert_eq!(rows.len(), 3, "{rows:?}");
    }

    #[test]
    fn approval_shows_the_whole_diff_in_the_history_and_the_prompt_for_the_input() {
        let mut app = app("I'll write it.", "draft");
        app.turn = Some(busy_turn());
        let history = app.on_event(axon_protocol::Event::ApprovalRequested {
            id: 1,
            change: Change::WriteFile {
                path: "a.txt".into(),
                old_text: None,
                new_text: (1..=30).map(|n| format!("{n}\n")).collect(),
            },
        });

        let screen = render(&app, &history, 40, 40);

        // The pane's 36 rows hold the end of the logo, the reply and all 31 diff rows.
        assert_eq!(screen.rows[4], "Agent I'll write it.");
        assert_eq!(screen.rows[5], "  a.txt (new file)");
        assert!(screen.rows[35].contains("30"), "{:?}", screen.rows[35]);
        assert!(!screen.rows.iter().any(|r| r.contains("more lines")));
        assert_eq!(screen.rows[36], "");
        assert!(screen.rows[37].contains("Waiting for approval"));
        assert_eq!(screen.rows[39], "  [y] accept  [n] reject  · Esc cancels");
        assert!(
            !screen.rows.iter().any(|r| r.contains("draft")),
            "the input is hidden"
        );

        app.approval = None;
        let screen = render(&app, &history, 40, 40);
        assert!(screen.rows.iter().any(|r| r == "  … 12 more lines"));
    }

    #[test]
    fn rejected_and_accepted_changes_commit_their_own_lines() {
        let rejected = Committed::ToolResult {
            name: "edit_file".into(),
            result: ToolResult::Rejected,
        };
        assert_eq!(
            text(committed(&rejected, 80, COMMITTED_DIFF_ROWS), 80),
            ["  ✗   edit_file  rejected"]
        );

        let accepted = Committed::Change(Change::WriteFile {
            path: "a.txt".into(),
            old_text: None,
            new_text: (1..=50).map(|n| format!("{n}\n")).collect(),
        });
        assert_eq!(
            text(committed(&accepted, 80, COMMITTED_DIFF_ROWS), 80).len(),
            COMMITTED_DIFF_ROWS
        );
    }

    #[test]
    fn a_command_shows_as_a_shell_line() {
        let command = Committed::Change(Change::RunCommand {
            command: "cargo test\n--quiet".into(),
        });

        assert_eq!(
            text(committed(&command, 80, COMMITTED_DIFF_ROWS), 80),
            ["  $   cargo test", "      --quiet"]
        );
    }

    #[test]
    fn command_results_say_how_they_ended() {
        let result = |head: &str, exit| {
            let line = Committed::ToolResult {
                name: "run_command".into(),
                result: ToolResult::Command {
                    head: head.into(),
                    cut: 0,
                    tail: String::new(),
                    exit,
                },
            };
            text(committed(&line, 80, COMMITTED_DIFF_ROWS), 80).join("\n")
        };

        assert_eq!(result("ok\n", Exit::Code(0)), "  ←   run_command  ok");
        assert_eq!(
            result("boom\nmore\n", Exit::Code(101)),
            "  ←   run_command  exit 101 · boom …"
        );
        assert_eq!(result("", Exit::Killed), "  ←   run_command  killed");
        assert_eq!(
            result("", Exit::TimedOut { secs: 120 }),
            "  ←   run_command  timed out after 120 s"
        );
        assert_eq!(result("", Exit::Cancelled), "  ←   run_command  cancelled");
    }
}
