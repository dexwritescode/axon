/// Interactive approval flow demo.
/// Run with: cargo run --example diff_approve_demo
///
/// Renders the full diff + prompt, waits for y/n, then:
///   y — clears the full diff and replaces it with the compact committed version
///   n — clears the full diff and leaves the terminal clean
use std::io::{Read, Write, stdout};

use axon::tui::diff::DiffRenderer;
use crossterm::{
    cursor,
    execute,
    terminal::{self, disable_raw_mode, enable_raw_mode},
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

    let diff_lines = renderer
        .render_full(&mut out, "src/main.rs", BEFORE, AFTER)
        .expect("render_full");

    // Approval prompt adds 2 lines after the diff.
    write!(out, "\r\n  Accept this diff? [y/n]\r\n").unwrap();
    out.flush().unwrap();

    let accepted = loop {
        let mut buf = [0u8; 1];
        std::io::stdin().read_exact(&mut buf).expect("read key");
        match buf[0] {
            b'y' | b'Y' => break true,
            b'n' | b'N' => break false,
            _ => {}
        }
    };

    // Clear the full diff + prompt, then render the outcome from the same position.
    execute!(
        out,
        cursor::MoveUp(diff_lines + 2),
        terminal::Clear(terminal::ClearType::FromCursorDown),
    )
    .unwrap();

    if accepted {
        renderer
            .render_committed(&mut out, "src/main.rs", BEFORE, AFTER)
            .expect("render_committed");
    }
    // rejected: nothing printed, terminal is clean

    disable_raw_mode().ok();
}
