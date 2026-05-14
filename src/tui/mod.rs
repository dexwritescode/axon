use std::io::{Stdout, stdout};

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs,
};
use crossterm::{
    cursor,
    event::{
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    style::Print,
    terminal::{self, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};

use crate::{
    app::{App, ChatMessage, PendingDiff},
    event::AppEvent,
    inference,
};

pub mod diff;
pub mod draw;

pub async fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();

    // Enable kitty keyboard enhancement so Shift+Enter is reported distinctly from Enter.
    // Push unconditionally — terminals that don't support it ignore the sequence.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );

    execute!(
        stdout,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    )?;
    draw::print_logo(&mut stdout)?;
    // Jump cursor so the live area (separator + input + status = 3 rows) sits at the
    // visible bottom. LOGO_ROWS + 1 is the minimum safe row below the logo.
    const LOGO_ROWS: u16 = 6;
    let (_, height) = terminal::size()?;
    let live_start = height.saturating_sub(3).max(LOGO_ROWS + 1);
    execute!(stdout, cursor::MoveTo(0, live_start))?;

    let result = run_loop(&mut stdout).await;

    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    // Move past the live area so the shell prompt appears below it.
    execute!(stdout, Print("\r\n"))?;

    result
}

async fn run_loop(stdout: &mut Stdout) -> Result<()> {
    let mut app = App::new();
    let renderer = diff::DiffRenderer::new();
    let mut events = EventStream::new();
    let mut tick = time::interval(Duration::from_millis(16));
    let mut inference_rx: Option<mpsc::Receiver<AppEvent>> = None;

    // Initial render — no clear needed on first draw.
    app.live_lines_to_top = draw::render_live(stdout, &app)?;

    while app.running {
        tokio::select! {
            _ = tick.tick() => {
                redraw(stdout, &mut app, &renderer)?;
            }
            Some(Ok(event)) = events.next() => {
                if let Event::Key(key) = event
                    && key.kind == KeyEventKind::Press
                    && let Some(rx) = handle_key(stdout, &mut app, key, &renderer)?
                {
                    inference_rx = Some(rx);
                }
            }
            Some(ev) = recv_inference(&mut inference_rx) => {
                handle_app_event(stdout, &mut app, ev, &mut inference_rx, &renderer)?;
            }
        }
    }

    Ok(())
}

async fn recv_inference(rx: &mut Option<mpsc::Receiver<AppEvent>>) -> Option<AppEvent> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

fn redraw(stdout: &mut Stdout, app: &mut App, renderer: &diff::DiffRenderer) -> Result<()> {
    draw::clear_live(stdout, app.live_lines_to_top)?;
    if let Some(ref pd) = app.pending_diff {
        let diff_lines = renderer.render_full(stdout, &pd.path, &pd.before, &pd.after)?;
        app.live_lines_to_top = draw::render_approval(stdout, app, diff_lines)?;
    } else {
        app.live_lines_to_top = draw::render_live(stdout, app)?;
    }
    Ok(())
}

fn handle_key(
    stdout: &mut Stdout,
    app: &mut App,
    key: KeyEvent,
    renderer: &diff::DiffRenderer,
) -> Result<Option<mpsc::Receiver<AppEvent>>> {
    // Approval mode: only y/n are meaningful; everything else is swallowed.
    if app.pending_diff.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let pd = app.pending_diff.take().unwrap();
                draw::clear_live(stdout, app.live_lines_to_top)?;
                renderer.render_committed(stdout, &pd.path, &pd.before, &pd.after)?;
                app.messages.push(ChatMessage::ToolCall {
                    name: format!("edit {}", pd.path),
                    args: serde_json::Value::Null,
                });
                if let Some(tx) = app.pending_approval.take() {
                    let _ = tx.send(true);
                }
                app.live_lines_to_top = draw::render_live(stdout, app)?;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.pending_diff = None;
                draw::clear_live(stdout, app.live_lines_to_top)?;
                if let Some(tx) = app.pending_approval.take() {
                    let _ = tx.send(false);
                }
                app.live_lines_to_top = draw::render_live(stdout, app)?;
            }
            KeyCode::Char('q') | KeyCode::Char('c')
                if key.modifiers == KeyModifiers::CONTROL =>
            {
                app.running = false;
            }
            _ => {}
        }
        return Ok(None);
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::CONTROL)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.running = false;
            Ok(None)
        }
        // Shift+Enter: proper kitty-protocol terminals send Enter+SHIFT; iTerm2 (and many
        // others) send raw LF (0x0A), which crossterm decodes as Ctrl+J.
        (KeyCode::Enter, KeyModifiers::SHIFT) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
            app.input.push('\n');
            redraw(stdout, app, renderer)?;
            Ok(None)
        }
        (KeyCode::Enter, _) => submit_message(stdout, app),
        (KeyCode::Backspace, _) => {
            app.input.pop();
            redraw(stdout, app, renderer)?;
            Ok(None)
        }
        (KeyCode::Up, _) => {
            // Arrow keys scroll the terminal's native scrollback — no app handling needed.
            Ok(None)
        }
        (KeyCode::Char(c), _) => {
            app.input.push(c);
            redraw(stdout, app, renderer)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn submit_message(stdout: &mut Stdout, app: &mut App) -> Result<Option<mpsc::Receiver<AppEvent>>> {
    let text = app.input.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }
    app.input.clear();

    // Commit user message to stdout permanently.
    draw::clear_live(stdout, app.live_lines_to_top)?;
    draw::commit_user(stdout, &text)?;

    app.messages.push(ChatMessage::User(text.clone()));

    // Render fresh live area (empty input, no streaming yet).
    app.live_lines_to_top = draw::render_live(stdout, app)?;

    let msg = ChatCompletionRequestUserMessageArgs::default()
        .content(text)
        .build()
        .expect("user message always valid");
    let messages = vec![ChatCompletionRequestMessage::User(msg)];

    Ok(Some(inference::spawn(
        app.client.clone(),
        messages,
        crate::tools::tool_schemas(),
        app.config.tool_approval.clone(),
    )))
}

fn handle_app_event(
    stdout: &mut Stdout,
    app: &mut App,
    event: AppEvent,
    inference_rx: &mut Option<mpsc::Receiver<AppEvent>>,
    renderer: &diff::DiffRenderer,
) -> Result<()> {
    match event {
        AppEvent::Token(delta) => {
            app.streaming_text.push_str(&delta);
            redraw(stdout, app, renderer)?;
        }
        AppEvent::ToolCall { name, args, .. } => {
            draw::clear_live(stdout, app.live_lines_to_top)?;
            draw::commit_agent(stdout, &app.streaming_text)?;
            app.streaming_text.clear();

            draw::commit_tool_call(stdout, &name, &args)?;
            app.messages.push(ChatMessage::ToolCall { name, args });

            app.live_lines_to_top = draw::render_live(stdout, app)?;
        }
        AppEvent::ToolResult { name, content } => {
            draw::clear_live(stdout, app.live_lines_to_top)?;
            draw::commit_tool_result(stdout, &name, &content)?;
            app.messages.push(ChatMessage::ToolResult { name, content });

            app.live_lines_to_top = draw::render_live(stdout, app)?;
        }
        AppEvent::FileDiff { path, before, after, approval } => {
            draw::clear_live(stdout, app.live_lines_to_top)?;
            draw::commit_agent(stdout, &app.streaming_text)?;
            app.streaming_text.clear();

            app.pending_diff = Some(PendingDiff { path, before, after });
            app.pending_approval = approval;

            let pd = app.pending_diff.as_ref().unwrap();
            let diff_lines = renderer.render_full(stdout, &pd.path, &pd.before, &pd.after)?;
            app.live_lines_to_top = draw::render_approval(stdout, app, diff_lines)?;
        }
        AppEvent::Done => {
            draw::clear_live(stdout, app.live_lines_to_top)?;
            if !app.streaming_text.is_empty() {
                draw::commit_agent(stdout, &app.streaming_text)?;
                app.messages
                    .push(ChatMessage::Agent(std::mem::take(&mut app.streaming_text)));
            }
            app.live_lines_to_top = draw::render_live(stdout, app)?;
            *inference_rx = None;
        }
    }
    Ok(())
}
