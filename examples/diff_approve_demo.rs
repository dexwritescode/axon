/// Interactive approval flow demo.
/// Run with: cargo run --example diff_approve_demo
///
/// Renders the full diff, waits for y/n, then shows what lands in the log.
use std::io::{Read, Write, stdout};

use axon::tui::diff::DiffRenderer;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

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

    renderer
        .render_full(&mut out, "src/main.rs", BEFORE, AFTER)
        .expect("render_full");

    write!(out, "\r\n  Accept this diff? [y/n] \r\n").unwrap();
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

    disable_raw_mode().ok();

    write!(out, "\r\n").unwrap();

    if accepted {
        renderer
            .render_committed(&mut out, "src/main.rs", BEFORE, AFTER)
            .expect("render_committed");
    } else {
        writeln!(out, "\r  (diff rejected — nothing committed)\r").unwrap();
        out.flush().unwrap();
    }
}
