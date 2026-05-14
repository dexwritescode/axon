/// Interactive approval flow demo.
/// Run with: cargo run --example diff_approve_demo
///
/// Shows the full diff + [y/n] prompt, waits for a keypress, then either
/// commits the compact diff to the log (y) or discards it silently (n).
use std::io::{Write, stdout};

use axon::{app::App, tui::diff::DiffRenderer, tui::draw};
use crossterm::{
    cursor,
    event::{read, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};

const BEFORE: &str = r#"fn greet(name: &str) {
    println!("hello, {}", name);
}

fn main() {
    greet("world");
}
"#;

const AFTER: &str = r#"fn greet(name: &str) -> String {
    format!("hello, {name}!")
}

fn farewell(name: &str) -> String {
    format!("goodbye, {name}!")
}

fn main() {
    let msg = greet("world");
    println!("{msg}");
    println!("{}", farewell("world"));
}
"#;

fn main() {
    enable_raw_mode().expect("raw mode required");
    let mut out = stdout();
    let renderer = DiffRenderer::new();
    let app = App::new();

    let diff_lines = renderer
        .render_full(&mut out, "src/main.rs", BEFORE, AFTER)
        .expect("render_full");
    let live_lines_to_top = draw::render_approval(&mut out, &app, diff_lines)
        .expect("render_approval");

    loop {
        let Ok(Event::Key(key)) = read() else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                draw::clear_live(&mut out, live_lines_to_top).expect("clear");
                renderer
                    .render_committed(&mut out, "src/main.rs", BEFORE, AFTER)
                    .expect("render_committed");
                execute!(out, cursor::Show).ok();
                break;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                draw::clear_live(&mut out, live_lines_to_top).expect("clear");
                writeln!(out, "\r  (diff rejected)\r").unwrap();
                out.flush().unwrap();
                execute!(out, cursor::Show).ok();
                break;
            }
            KeyCode::Char('q') | KeyCode::Char('c') => {
                execute!(out, cursor::Show).ok();
                break;
            }
            _ => {}
        }
    }

    disable_raw_mode().ok();
}
