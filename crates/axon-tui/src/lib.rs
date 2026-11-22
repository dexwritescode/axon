//! Terminal frontend: draws a session full screen, on the alternate screen.
#![allow(clippy::disallowed_methods, reason = "the TUI owns stdout")]

mod app;
mod diff;
mod draw;
mod prompt;
mod theme;

use std::io::{self, Write};
use std::time::{Duration, Instant};

pub use app::Settings;

use app::{Action, App, Committed};
use axon_protocol::{Command, Event, Resume, SessionError, SessionFactory, SessionHandle};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event as TermEvent, EventStream, KeyEvent, KeyEventKind, KeyboardEnhancementFlags,
        MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use prompt::Prompt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("terminal: {0}")]
    Io(#[from] io::Error),
}

/// Run the session `resume` names in the terminal until the user quits,
/// then say how to resume it.
pub async fn run(
    factory: &dyn SessionFactory,
    settings: Settings,
    resume: Resume,
) -> Result<(), TuiError> {
    let session = factory.new_session(std::env::current_dir()?, resume, vec![])?;
    let id = session.id.clone();
    let saved = {
        let _screen = Screen::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        run_loop(&mut terminal, session, settings).await?
    };
    if saved {
        writeln!(io::stdout(), "resume: axon --resume {id}")?;
    }
    Ok(())
}

/// The alternate screen in raw mode, with the mouse captured, for as long as
/// it lives; dropping it restores the terminal, also when unwinding from a panic.
struct Screen;

impl Screen {
    fn enter() -> io::Result<Self> {
        // Built first, so a failure below still restores what was set.
        let screen = Self;
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        // Unsupported on the legacy Windows console, where pastes arrive as keys.
        let _ = execute!(io::stdout(), EnableBracketedPaste);
        // Lets terminals that support it report Shift+Enter apart from Enter.
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

type Tty = Terminal<CrosstermBackend<io::Stdout>>;

/// How long quitting waits for the session to save a cancelled turn.
const QUIT_WAIT: Duration = Duration::from_secs(2);

/// How often the activity row animates.
const TICK: Duration = Duration::from_millis(100);

/// Something the loop woke up for.
enum Input {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Session(Option<Event>),
    /// Time to animate the activity row.
    Tick,
    /// Anything else from the terminal, such as a resize.
    Redraw,
}

/// Runs until the user quits; whether the session has anything saved.
async fn run_loop(
    terminal: &mut Tty,
    session: SessionHandle,
    settings: Settings,
) -> Result<bool, TuiError> {
    let SessionHandle {
        commands,
        mut events,
        ..
    } = session;
    let mut saved = false;
    let mut app = App {
        settings,
        ..App::default()
    };
    let mut keys = EventStream::new();
    let mut ticks = tokio::time::interval(TICK);
    let mut history = Vec::new();

    redraw(terminal, &mut app, &history)?;

    loop {
        let input = tokio::select! {
            key = keys.next() => match key {
                Some(Ok(TermEvent::Key(key))) if key.kind == KeyEventKind::Press => Input::Key(key),
                Some(Ok(TermEvent::Mouse(mouse))) => match mouse.kind {
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => Input::Mouse(mouse),
                    // Moves and clicks change nothing yet; skip the redraw.
                    _ => continue,
                },
                Some(Ok(TermEvent::Paste(text))) => Input::Paste(text),
                Some(Ok(_)) => Input::Redraw,
                Some(Err(e)) => return Err(e.into()),
                None => break,
            },
            event = events.recv() => Input::Session(event),
            _ = ticks.tick(), if app.turn.is_some() => Input::Tick,
        };
        if let Input::Session(Some(Event::TurnEnded { .. } | Event::History { .. })) = input {
            saved = true;
        }
        let Some(committed) = step(&mut app, &commands, input).await else {
            break;
        };
        history.extend(committed);
        redraw(terminal, &mut app, &history)?;
    }
    // Ends the session; a running turn is cancelled and saved before its events close.
    drop(commands);
    let drain = async {
        while let Some(event) = events.recv().await {
            saved |= matches!(event, Event::TurnEnded { .. });
        }
    };
    let _ = tokio::time::timeout(QUIT_WAIT, drain).await;
    Ok(saved)
}

/// Apply one input: the lines to commit, or `None` when the TUI should exit.
async fn step(
    app: &mut App,
    commands: &mpsc::Sender<Command>,
    input: Input,
) -> Option<Vec<Committed>> {
    match input {
        Input::Key(key) => match app.on_key(key) {
            Some(Action::Quit) => None,
            Some(Action::Cancel) => {
                commands.send(Command::Cancel).await.ok()?;
                Some(Vec::new())
            }
            Some(Action::Answer { id, approve }) => {
                commands
                    .send(Command::Approval { id, approve })
                    .await
                    .ok()?;
                Some(Vec::new())
            }
            Some(Action::Submit(prompt)) => Some(vec![submit(commands, &prompt).await?]),
            None => Some(Vec::new()),
        },
        Input::Mouse(mouse) => {
            app.on_mouse(mouse);
            Some(Vec::new())
        }
        Input::Paste(text) => {
            app.on_paste(&text);
            Some(Vec::new())
        }
        Input::Session(event) => {
            let mut committed = app.on_event(event?);
            if let Some(Action::Submit(prompt)) = app.send_queued() {
                committed.push(submit(commands, &prompt).await?);
            }
            Some(committed)
        }
        Input::Tick => {
            app.on_tick(Instant::now());
            Some(Vec::new())
        }
        Input::Redraw => Some(Vec::new()),
    }
}

/// Send `prompt` in full; its history line shows long pastes as labels.
async fn submit(commands: &mpsc::Sender<Command>, prompt: &Prompt) -> Option<Committed> {
    let text = prompt.expanded();
    commands.send(Command::Prompt { text }).await.ok()?;
    Some(Committed::User(prompt.shown()))
}

/// Draw the screen and remember the history pane's size for scrolling.
fn redraw(terminal: &mut Tty, app: &mut App, history: &[Committed]) -> io::Result<()> {
    let mut view = app.view;
    terminal.draw(|frame| view = draw::screen(frame, app, history))?;
    app.view = view;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> Input {
        Input::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn app_with_input(text: &str) -> App {
        App {
            input: text.into(),
            ..App::default()
        }
    }

    #[tokio::test]
    async fn submit_sends_a_prompt_and_commits_the_user_line() {
        let (commands, mut sent) = mpsc::channel(1);
        let mut app = app_with_input("hi");

        let committed = step(&mut app, &commands, key(KeyCode::Enter)).await;

        assert_eq!(committed, Some(vec![Committed::User("hi".into())]));
        assert!(matches!(sent.recv().await, Some(Command::Prompt { text }) if text == "hi"));
    }

    #[tokio::test]
    async fn submit_sends_long_pastes_in_full_and_shows_them_as_labels() {
        let (commands, mut sent) = mpsc::channel(1);
        let mut app = app_with_input("see ");
        app.settings.paste_token_chars = 5;
        app.on_paste("long paste");

        let committed = step(&mut app, &commands, key(KeyCode::Enter)).await;

        let shown = Committed::User("see [pasted 10 chars]".into());
        assert_eq!(committed, Some(vec![shown]));
        assert!(
            matches!(sent.recv().await, Some(Command::Prompt { text }) if text == "see long paste")
        );
    }

    #[tokio::test]
    async fn submit_to_a_stopped_session_exits() {
        let (commands, sent) = mpsc::channel(1);
        drop(sent);
        let mut app = app_with_input("hi");

        assert_eq!(step(&mut app, &commands, key(KeyCode::Enter)).await, None);
    }

    #[tokio::test]
    async fn session_events_commit_through_the_app() {
        let (commands, _sent) = mpsc::channel(1);
        let mut app = App::default();
        let delta = Event::TextDelta {
            text: "Hello".into(),
        };
        let end = Event::TurnEnded {
            stop: axon_protocol::StopReason::EndTurn,
        };

        assert_eq!(
            step(&mut app, &commands, Input::Session(Some(delta))).await,
            Some(Vec::new())
        );
        assert_eq!(
            step(&mut app, &commands, Input::Session(Some(end))).await,
            Some(vec![Committed::Agent("Hello".into())])
        );
    }

    #[tokio::test]
    async fn ctrl_s_cancels_and_the_queue_goes_out_when_the_turn_ends() {
        let (commands, mut sent) = mpsc::channel(3);
        let mut app = app_with_input("hi");
        step(&mut app, &commands, key(KeyCode::Enter)).await;
        app.input = "now".into();

        let ctrl_s = Input::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        step(&mut app, &commands, ctrl_s).await;
        let end = Event::TurnEnded {
            stop: axon_protocol::StopReason::Cancelled,
        };
        let committed = step(&mut app, &commands, Input::Session(Some(end))).await;

        assert_eq!(
            committed,
            Some(vec![Committed::Cancelled, Committed::User("now".into())])
        );
        sent.recv().await;
        assert!(matches!(sent.recv().await, Some(Command::Cancel)));
        assert!(matches!(sent.recv().await, Some(Command::Prompt { text }) if text == "now"));
    }

    #[tokio::test]
    async fn esc_during_a_turn_sends_cancel() {
        let (commands, mut sent) = mpsc::channel(2);
        let mut app = app_with_input("hi");
        step(&mut app, &commands, key(KeyCode::Enter)).await;

        assert_eq!(
            step(&mut app, &commands, key(KeyCode::Esc)).await,
            Some(Vec::new())
        );

        sent.recv().await;
        assert!(matches!(sent.recv().await, Some(Command::Cancel)));
    }

    #[tokio::test]
    async fn closed_session_and_quit_exit() {
        let (commands, _sent) = mpsc::channel(1);
        let mut app = App::default();

        assert_eq!(step(&mut app, &commands, Input::Session(None)).await, None);
        let quit = Input::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(step(&mut app, &commands, quit).await, None);
    }
}
