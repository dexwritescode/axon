use std::io::Stdout;

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs,
};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;
use tokio::time::{self, Duration};

use crate::{
    app::{App, ChatMessage},
    event::AppEvent,
    inference,
};

pub mod draw;

pub async fn run() -> Result<()> {
    let mut terminal = setup()?;
    let result = run_loop(&mut terminal).await;
    restore(&mut terminal)?;
    result
}

fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

async fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new();
    let mut events = EventStream::new();
    let mut tick = time::interval(Duration::from_millis(16));
    let mut inference_rx: Option<mpsc::Receiver<AppEvent>> = None;

    while app.running {
        tokio::select! {
            _ = tick.tick() => {
                terminal.draw(|f| draw::draw(f, &app))?;
            }
            Some(Ok(event)) = events.next() => {
                if let Event::Key(key) = event
                    && let Some(rx) = handle_key(&mut app, key)
                {
                    inference_rx = Some(rx);
                }
            }
            Some(ev) = recv_inference(&mut inference_rx) => {
                handle_app_event(&mut app, ev, &mut inference_rx);
            }
        }
    }

    Ok(())
}

/// Returns `pending()` when there is no active receiver, keeping the select! arm dormant.
async fn recv_inference(rx: &mut Option<mpsc::Receiver<AppEvent>>) -> Option<AppEvent> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<mpsc::Receiver<AppEvent>> {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.running = false;
            None
        }
        (KeyCode::Enter, _) => submit_message(app),
        (KeyCode::Backspace, _) => {
            app.input.pop();
            None
        }
        (KeyCode::Char(c), _) => {
            app.input.push(c);
            None
        }
        _ => None,
    }
}

fn submit_message(app: &mut App) -> Option<mpsc::Receiver<AppEvent>> {
    let text = app.input.trim().to_string();
    if text.is_empty() {
        return None;
    }
    app.input.clear();
    app.messages.push(ChatMessage::User(text.clone()));

    let msg = ChatCompletionRequestUserMessageArgs::default()
        .content(text)
        .build()
        .expect("user message always valid");
    let messages = vec![ChatCompletionRequestMessage::User(msg)];

    Some(inference::spawn(
        app.client.clone(),
        messages,
        vec![],
        app.config.tool_approval.clone(),
    ))
}

fn handle_app_event(
    app: &mut App,
    event: AppEvent,
    inference_rx: &mut Option<mpsc::Receiver<AppEvent>>,
) {
    match event {
        AppEvent::Token(delta) => match app.messages.last_mut() {
            Some(ChatMessage::Agent(text)) => text.push_str(&delta),
            _ => app.messages.push(ChatMessage::Agent(delta)),
        },
        AppEvent::ToolCall { name, args, .. } => {
            app.messages.push(ChatMessage::ToolCall { name, args });
        }
        AppEvent::ToolResult { name, content } => {
            app.messages.push(ChatMessage::ToolResult { name, content });
        }
        AppEvent::Done => {
            *inference_rx = None;
        }
    }
}
