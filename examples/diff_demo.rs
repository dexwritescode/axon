/// Visual smoke-test for the diff renderer.
/// Run with: cargo run --example diff_demo
///
/// Uses a hardcoded Rust before/after to exercise syntax highlighting,
/// red/green background tints, and side-by-side line numbers.
use std::io::stdout;

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
    // Raw mode is best-effort — fails when stdout is not a TTY (e.g., CI).
    // The diff still renders correctly without it since we use \r\n explicitly.
    let raw = enable_raw_mode().is_ok();
    let mut out = stdout();
    let renderer = DiffRenderer::new();
    renderer
        .render_full(&mut out, "src/main.rs", BEFORE, AFTER)
        .expect("render diff");
    if raw {
        disable_raw_mode().ok();
    }
}
