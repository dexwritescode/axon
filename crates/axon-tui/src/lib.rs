//! Terminal frontend: draws a session inline in the terminal's scrollback.

mod app;
mod draw;

use std::io::{self, Stdout, Write};

use app::{Action, App, Committed};
use axon_protocol::{Command, SessionError, SessionFactory, SessionHandle};
use crossterm::{
    cursor,
    event::{
        Event as TermEvent, EventStream, KeyEventKind, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    style::Print,
    terminal::{self, ClearType, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;

#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("terminal: {0}")]
    Io(#[from] io::Error),
}

/// Run one session in the terminal until the user quits.
pub async fn run(factory: &dyn SessionFactory) -> Result<(), TuiError> {
    let session = factory.new_session()?;
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    // Lets terminals that support it report Shift+Enter apart from Enter.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );

    let result = run_loop(&mut stdout, session).await;

    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    execute!(stdout, Print("\r\n"))?;
    result
}

async fn run_loop(out: &mut Stdout, session: SessionHandle) -> Result<(), TuiError> {
    let SessionHandle {
        commands,
        mut events,
    } = session;
    let mut app = App::default();
    let mut keys = EventStream::new();

    execute!(out, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0))?;
    draw::print_logo(out)?;
    // Start the live area at the bottom of the screen, below the logo.
    let (_, height) = terminal::size()?;
    let logo_rows = draw::LOGO.len() as u16;
    execute!(
        out,
        cursor::MoveTo(0, height.saturating_sub(2).max(logo_rows + 1))
    )?;
    let mut lines_to_top = redraw(out, &app, 0, &[])?;

    loop {
        let committed = tokio::select! {
            key = keys.next() => match key {
                Some(Ok(TermEvent::Key(key))) if key.kind == KeyEventKind::Press => {
                    match app.on_key(key) {
                        Some(Action::Quit) => return Ok(()),
                        Some(Action::Submit(text)) => {
                            let prompt = Command::Prompt { text: text.clone() };
                            if commands.send(prompt).await.is_err() {
                                return Ok(());
                            }
                            vec![Committed::User(text)]
                        }
                        None => Vec::new(),
                    }
                }
                Some(Ok(_)) => Vec::new(),
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            event = events.recv() => match event {
                Some(event) => app.on_event(event),
                None => return Ok(()),
            },
        };
        lines_to_top = redraw(out, &app, lines_to_top, &committed)?;
    }
}

/// Replace the live area: commit finished lines above it, then draw it again.
fn redraw(
    out: &mut impl Write,
    app: &App,
    lines_to_top: u16,
    committed: &[Committed],
) -> io::Result<u16> {
    draw::clear_live(out, lines_to_top)?;
    for line in committed {
        draw::commit(out, line)?;
    }
    let (width, height) = terminal::size()?;
    draw::render_live(out, app, width, height)
}
