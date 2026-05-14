/// Approval flow demo for the diff renderer.
/// Run with: cargo run --example diff_approve_demo
///
/// Shows three sequential scenarios without requiring user input:
///   1. Diff arrives   — full diff + [y/n] approval prompt
///   2. User presses y — compact diff committed to permanent log
///   3. User presses n — diff discarded, nothing committed
use std::io::{Write, stdout};

use axon::{app::App, tui::diff::DiffRenderer, tui::draw};
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

fn banner(out: &mut impl Write, label: &str) {
    let bar = "─".repeat(60);
    writeln!(out, "\r\n{bar}\r\n  {label}\r\n{bar}\r").unwrap();
}

fn main() {
    let raw = enable_raw_mode().is_ok();
    let mut out = stdout();
    let renderer = DiffRenderer::new();
    let app = App::new();

    // ── scenario 1: diff arrives ───────────────────────────────────────────────
    banner(&mut out, "1/3  diff arrives — waiting for y/n");

    let diff_lines = renderer
        .render_full(&mut out, "src/main.rs", BEFORE, AFTER)
        .expect("render_full");

    draw::render_approval(&mut out, &app, diff_lines).expect("render_approval");

    // ── scenario 2: user presses y ────────────────────────────────────────────
    banner(&mut out, "2/3  user pressed y — committed to log");

    renderer
        .render_committed(&mut out, "src/main.rs", BEFORE, AFTER)
        .expect("render_committed");

    // ── scenario 3: user presses n ────────────────────────────────────────────
    banner(&mut out, "3/3  user pressed n — rejected, nothing committed");

    writeln!(out, "\r  (diff discarded — no output)\r").unwrap();
    out.flush().unwrap();

    if raw {
        disable_raw_mode().ok();
    }
}
