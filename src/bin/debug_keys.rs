/// Prints raw crossterm key events so we can see exactly what sequence is produced
/// by Shift+Enter (or any other key) before and after keyboard enhancement is pushed.
///
/// Usage:  cargo run --bin debug_keys
use crossterm::{
    event::{
        read, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    style::Print,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io::stdout;

fn main() -> std::io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();

    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );

    execute!(
        stdout,
        Print("Key debugger — press keys to see raw events. Ctrl+C to quit.\r\n"),
    )?;

    loop {
        match read()? {
            Event::Key(key) => {
                execute!(
                    stdout,
                    Print(format!(
                        "code={:?}  modifiers={:?}  kind={:?}\r\n",
                        key.code, key.modifiers, key.kind
                    )),
                )?;
                if key.code == crossterm::event::KeyCode::Char('c')
                    && key.modifiers == crossterm::event::KeyModifiers::CONTROL
                {
                    break;
                }
            }
            other => {
                execute!(stdout, Print(format!("other: {other:?}\r\n")))?;
            }
        }
    }

    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    Ok(())
}
