//! Every colour the TUI draws with, named by what it marks.
//! Diff colours are GitHub's dark-mode ones.

use ratatui::style::Color;
use two_face::theme::EmbeddedThemeName;

pub(crate) const USER: Color = Color::Green;
/// The agent's label, the logo and the spinner.
pub(crate) const AGENT: Color = Color::Cyan;
/// Errors, cancelled turns and failed tools.
pub(crate) const ERROR: Color = Color::Red;
/// A turn that finished.
pub(crate) const DONE: Color = Color::Green;
/// A change the user rejected: their choice, not an error.
pub(crate) const REJECTED: Color = Color::Rgb(232, 163, 61);
/// Tool calls and their results.
pub(crate) const TOOL: Color = Color::DarkGray;
/// Secondary text: activity, the separator, diff notes.
pub(crate) const MUTED: Color = Color::DarkGray;
pub(crate) const INPUT: Color = Color::White;
pub(crate) const ACCEPT: Color = Color::Green;
pub(crate) const REJECT: Color = Color::Red;
pub(crate) const DIFF_ADDED: Color = Color::Rgb(63, 185, 80);
pub(crate) const DIFF_ADDED_BG: Color = Color::Rgb(18, 38, 30);
/// The words that changed within an added line.
pub(crate) const DIFF_ADDED_WORDS_BG: Color = Color::Rgb(29, 87, 45);
pub(crate) const DIFF_REMOVED: Color = Color::Rgb(248, 81, 73);
pub(crate) const DIFF_REMOVED_BG: Color = Color::Rgb(37, 23, 28);
/// The words that changed within a removed line.
pub(crate) const DIFF_REMOVED_WORDS_BG: Color = Color::Rgb(121, 46, 46);
pub(crate) const DIFF_CONTEXT: Color = Color::DarkGray;
/// Colours for code in diffs.
pub(crate) const SYNTAX_THEME: EmbeddedThemeName = EmbeddedThemeName::OneHalfDark;
