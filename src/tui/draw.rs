use std::io::{Stdout, Write};

use crossterm::{
    cursor, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, ClearType},
};

use crate::app::{App, ChatMessage};

const INDENT: &str = "      ";
const TRUNCATE_AT: usize = 120;

// ── live area ─────────────────────────────────────────────────────────────────

/// Move cursor to the top of the live area and clear everything below.
/// Must be called before render_live on every redraw after the first.
/// Does NOT flush — caller must flush after the subsequent render_live to avoid
/// a blank-frame flash between clear and repaint.
pub fn clear_live(stdout: &mut Stdout, lines_to_top: u16) -> std::io::Result<()> {
    queue!(stdout, cursor::Hide, cursor::MoveToColumn(0))?;
    if lines_to_top > 0 {
        queue!(stdout, cursor::MoveUp(lines_to_top))?;
    }
    queue!(stdout, terminal::Clear(ClearType::FromCursorDown))
}

/// Render the live area (streaming text + separator + input + status).
/// Assumes the cursor is at the start of the live area (col 0).
/// Leaves the cursor on the input line for display.
/// Returns the new `live_lines_to_top` for the next clear_live call.
pub fn render_live(stdout: &mut Stdout, app: &App) -> std::io::Result<u16> {
    let (width, _) = terminal::size()?;
    let mut streaming_lines = 0u16;

    // Streaming agent text
    if !app.streaming_text.is_empty() {
        let mut parts = app.streaming_text.split('\n');
        if let Some(first) = parts.next() {
            queue!(
                stdout,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print("Agent "),
                SetAttribute(Attribute::Reset),
                ResetColor,
                Print(first),
                Print("\r\n"),
            )?;
            streaming_lines += 1;
        }
        for cont in parts {
            queue!(stdout, Print(INDENT), Print(cont), Print("\r\n"))?;
            streaming_lines += 1;
        }
    }

    // Separator
    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(width as usize)),
        ResetColor,
        Print("\r\n"),
    )?;

    // Input line (multiline-aware)
    let input_parts: Vec<&str> = app.input.split('\n').collect();
    let input_line_count = input_parts.len() as u16;
    queue!(
        stdout,
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print("> "),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Print(input_parts[0]),
        Print("\r\n"),
    )?;
    for cont in &input_parts[1..] {
        queue!(stdout, Print("  "), Print(cont), Print("\r\n"))?;
    }

    // Status line — last line, no trailing \r\n
    let backend = &app.config.backend;
    let model = if backend.model.is_empty() {
        "(no model)".to_string()
    } else {
        backend.model.clone()
    };
    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print(format!(
            "  {}  │  {}  │  tools: {}",
            backend.base_url,
            model,
            app.config.tool_approval.as_str()
        )),
        ResetColor,
    )?;

    // Move cursor back up to the last input line for display.
    let last_input = input_parts.last().copied().unwrap_or("");
    let cursor_col = 2 + last_input.len() as u16;
    queue!(
        stdout,
        cursor::MoveUp(1),
        cursor::MoveToColumn(cursor_col),
        cursor::Show,
    )?;
    stdout.flush()?;

    // lines_to_top = streaming lines + 1 (separator) + extra input lines above cursor.
    Ok(streaming_lines + input_line_count)
}

// ── startup ───────────────────────────────────────────────────────────────────

pub fn print_logo(stdout: &mut Stdout) -> std::io::Result<()> {
    let lines = [
        " █████╗ ██╗  ██╗ ██████╗ ███╗   ██╗",
        "██╔══██╗╚██╗██╔╝██╔═══██╗████╗  ██║",
        "███████║ ╚███╔╝ ██║   ██║██╔██╗ ██║",
        "██╔══██║ ██╔██╗ ██║   ██║██║╚██╗██║",
        "██║  ██║██╔╝ ██╗╚██████╔╝██║ ╚████║",
        "╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝",
    ];
    for line in &lines {
        queue!(
            stdout,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print(line),
            SetAttribute(Attribute::Reset),
            ResetColor,
            Print("\r\n"),
        )?;
    }
    stdout.flush()
}

// ── approval prompt ───────────────────────────────────────────────────────────

/// Render the separator + [y/n] prompt + status below a diff that is waiting
/// for user approval. Call immediately after `DiffRenderer::render_full`.
/// Returns the new `live_lines_to_top` for the next `clear_live` call.
pub fn render_approval(stdout: &mut Stdout, app: &App, diff_lines: u16) -> std::io::Result<u16> {
    let (width, _) = terminal::size()?;

    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(width as usize)),
        ResetColor,
        Print("\r\n"),
    )?;

    queue!(
        stdout,
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print("  [y]"),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(" accept    "),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print("[n]"),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(" reject"),
        ResetColor,
        Print("\r\n"),
    )?;

    let backend = &app.config.backend;
    let model = if backend.model.is_empty() {
        "(no model)".to_string()
    } else {
        backend.model.clone()
    };
    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print(format!(
            "  {}  │  {}  │  tools: {}",
            backend.base_url,
            model,
            app.config.tool_approval.as_str()
        )),
        ResetColor,
    )?;

    queue!(stdout, cursor::MoveUp(1), cursor::MoveToColumn(2), cursor::Show)?;
    stdout.flush()?;

    // diff_lines are above the separator; cursor is on the prompt line (separator + prompt above).
    Ok(diff_lines + 1)
}

// ── committed output ──────────────────────────────────────────────────────────

pub fn commit_user(stdout: &mut Stdout, text: &str) -> std::io::Result<()> {
    let mut parts = text.split('\n');
    if let Some(first) = parts.next() {
        queue!(
            stdout,
            SetForegroundColor(Color::Green),
            SetAttribute(Attribute::Bold),
            Print("You   "),
            SetAttribute(Attribute::Reset),
            ResetColor,
            Print(first),
            Print("\r\n"),
        )?;
    }
    for cont in parts {
        queue!(stdout, Print(INDENT), Print(cont), Print("\r\n"))?;
    }
    stdout.flush()
}

pub fn commit_agent(stdout: &mut Stdout, text: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut parts = text.split('\n');
    if let Some(first) = parts.next() {
        queue!(
            stdout,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            Print("Agent "),
            SetAttribute(Attribute::Reset),
            ResetColor,
            Print(first),
            Print("\r\n"),
        )?;
    }
    for cont in parts {
        queue!(stdout, Print(INDENT), Print(cont), Print("\r\n"))?;
    }
    stdout.flush()
}

pub fn commit_tool_call(
    stdout: &mut Stdout,
    name: &str,
    args: &serde_json::Value,
) -> std::io::Result<()> {
    let args_str = serde_json::to_string(args).unwrap_or_default();
    let args_display = truncate(&args_str, TRUNCATE_AT);
    queue!(
        stdout,
        SetForegroundColor(Color::Yellow),
        Print("  →   "),
        SetAttribute(Attribute::Bold),
        Print(name),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(format!("  {args_display}")),
        ResetColor,
        Print("\r\n"),
    )?;
    stdout.flush()
}

pub fn commit_tool_result(stdout: &mut Stdout, name: &str, content: &str) -> std::io::Result<()> {
    let first_line = content.lines().next().unwrap_or("");
    let truncated = truncate(first_line, TRUNCATE_AT);
    let suffix = if content.contains('\n') || content.len() > TRUNCATE_AT {
        " …"
    } else {
        ""
    };
    queue!(
        stdout,
        SetForegroundColor(Color::Magenta),
        Print("  ←   "),
        SetAttribute(Attribute::Bold),
        Print(name),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(format!("  {truncated}{suffix}")),
        ResetColor,
        Print("\r\n"),
    )?;
    stdout.flush()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        s
    } else {
        let mut idx = max_chars;
        while !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

// ── kept for inference context display (unused in rendering) ──────────────────
#[allow(dead_code)]
pub fn format_message(msg: &ChatMessage) -> String {
    match msg {
        ChatMessage::User(t) => format!("You   {t}"),
        ChatMessage::Agent(t) => format!("Agent {t}"),
        ChatMessage::ToolCall { name, args } => format!("  →   {name}  {args}"),
        ChatMessage::ToolResult { name, content } => format!("  ←   {name}  {content}"),
    }
}
