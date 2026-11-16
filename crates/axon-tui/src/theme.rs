//! Every colour the TUI draws with, named by what it marks.
//! Diff colours are GitHub's dark-mode ones.

use crossterm::style::Color;
use two_face::theme::EmbeddedThemeName;

pub(crate) const USER: Color = Color::Green;
/// The agent's label, the logo and the spinner.
pub(crate) const AGENT: Color = Color::Cyan;
/// Errors, cancelled turns and failed tools.
pub(crate) const ERROR: Color = Color::Red;
/// A change the user rejected: their choice, not an error.
pub(crate) const REJECTED: Color = Color::Rgb {
    r: 232,
    g: 163,
    b: 61,
};
/// Tool calls and their results.
pub(crate) const TOOL: Color = Color::DarkGrey;
/// Secondary text: activity, the separator, diff notes.
pub(crate) const MUTED: Color = Color::DarkGrey;
pub(crate) const INPUT: Color = Color::White;
pub(crate) const ACCEPT: Color = Color::Green;
pub(crate) const REJECT: Color = Color::Red;
pub(crate) const DIFF_ADDED: Color = Color::Rgb {
    r: 63,
    g: 185,
    b: 80,
};
pub(crate) const DIFF_ADDED_BG: Color = Color::Rgb {
    r: 18,
    g: 38,
    b: 30,
};
/// The words that changed within an added line.
pub(crate) const DIFF_ADDED_WORDS_BG: Color = Color::Rgb {
    r: 29,
    g: 87,
    b: 45,
};
pub(crate) const DIFF_REMOVED: Color = Color::Rgb {
    r: 248,
    g: 81,
    b: 73,
};
pub(crate) const DIFF_REMOVED_BG: Color = Color::Rgb {
    r: 37,
    g: 23,
    b: 28,
};
/// The words that changed within a removed line.
pub(crate) const DIFF_REMOVED_WORDS_BG: Color = Color::Rgb {
    r: 121,
    g: 46,
    b: 46,
};
pub(crate) const DIFF_CONTEXT: Color = Color::DarkGrey;
/// Colours for code in diffs.
pub(crate) const SYNTAX_THEME: EmbeddedThemeName = EmbeddedThemeName::OneHalfDark;
