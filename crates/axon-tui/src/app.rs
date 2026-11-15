//! UI state and how keys and session events change it. No terminal I/O.

use std::mem;

use axon_protocol::{Event, StopReason, ToolStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default)]
pub(crate) struct App {
    pub(crate) input: String,
    /// The reply streaming in for the current turn.
    pub(crate) streaming_text: String,
    /// A turn is running; Enter does nothing until it ends.
    pub(crate) busy: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Submit(String),
    Quit,
}

/// A finished line for the scrollback.
#[derive(Debug, PartialEq)]
pub(crate) enum Committed {
    User(String),
    Agent(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        name: String,
        status: ToolStatus,
        output: String,
    },
    Error(String),
}

impl App {
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c' | 'q'), KeyModifiers::CONTROL) => Some(Action::Quit),
            // Terminals without the kitty keyboard protocol send Shift+Enter as
            // a line feed, which crossterm reports as Ctrl+J.
            (KeyCode::Enter, KeyModifiers::SHIFT) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.input.push('\n');
                None
            }
            (KeyCode::Enter, _) => self.submit(),
            (KeyCode::Backspace, _) => {
                self.input.pop();
                None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    fn submit(&mut self) -> Option<Action> {
        let text = self.input.trim();
        if self.busy || text.is_empty() {
            return None;
        }
        let text = text.to_string();
        self.input.clear();
        self.busy = true;
        Some(Action::Submit(text))
    }

    pub(crate) fn on_event(&mut self, event: Event) -> Vec<Committed> {
        match event {
            Event::TextDelta { text } => {
                self.streaming_text.push_str(&text);
                Vec::new()
            }
            Event::ToolCallStarted { name, args } => {
                let mut committed = self.take_reply();
                committed.push(Committed::ToolCall { name, args });
                committed
            }
            Event::ToolCallFinished {
                name,
                status,
                output,
            } => vec![Committed::ToolResult {
                name,
                status,
                output,
            }],
            Event::Error { message } => {
                let mut committed = self.take_reply();
                committed.push(Committed::Error(message));
                committed
            }
            Event::TurnEnded { stop } => {
                self.busy = false;
                let mut committed = self.take_reply();
                if stop == StopReason::MaxTurnRequests {
                    committed.push(Committed::Error(
                        "stopped: too many model requests for one prompt".into(),
                    ));
                }
                committed
            }
        }
    }

    fn take_reply(&mut self) -> Vec<Committed> {
        let text = mem::take(&mut self.streaming_text);
        let text = text.trim();
        if text.is_empty() {
            Vec::new()
        } else {
            vec![Committed::Agent(text.into())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    fn delta(text: &str) -> Event {
        Event::TextDelta { text: text.into() }
    }

    const END: Event = Event::TurnEnded {
        stop: StopReason::EndTurn,
    };

    #[test]
    fn enter_submits_the_trimmed_input_and_marks_busy() {
        let mut app = App::default();
        type_text(&mut app, " hi ");

        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Some(Action::Submit("hi".into()))
        );
        assert_eq!(app.input, "");
        assert!(app.busy);
    }

    #[test]
    fn enter_with_blank_input_does_nothing() {
        let mut app = App::default();
        type_text(&mut app, "  ");

        assert_eq!(app.on_key(key(KeyCode::Enter)), None);
        assert!(!app.busy);
    }

    #[test]
    fn enter_while_busy_keeps_the_input() {
        let mut app = App {
            busy: true,
            ..App::default()
        };
        type_text(&mut app, "next");

        assert_eq!(app.on_key(key(KeyCode::Enter)), None);
        assert_eq!(app.input, "next");
    }

    #[test]
    fn shift_enter_and_ctrl_j_insert_a_newline() {
        let mut app = App::default();
        type_text(&mut app, "a");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut app, "b");
        app.on_key(ctrl('j'));

        assert_eq!(app.input, "a\nb\n");
    }

    #[test]
    fn backspace_removes_the_last_char() {
        let mut app = App::default();
        type_text(&mut app, "hé");
        app.on_key(key(KeyCode::Backspace));

        assert_eq!(app.input, "h");
    }

    #[test]
    fn shift_types_and_other_modifiers_do_not() {
        let mut app = App::default();
        app.on_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        app.on_key(ctrl('a'));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));

        assert_eq!(app.input, "A");
    }

    #[test]
    fn ctrl_c_and_ctrl_q_quit() {
        let mut app = App::default();

        assert_eq!(app.on_key(ctrl('c')), Some(Action::Quit));
        assert_eq!(app.on_key(ctrl('q')), Some(Action::Quit));
    }

    #[test]
    fn deltas_stream_then_turn_end_commits_the_reply() {
        let mut app = App {
            busy: true,
            ..App::default()
        };

        assert!(app.on_event(delta("Hel")).is_empty());
        assert!(app.on_event(delta("lo")).is_empty());
        assert_eq!(app.streaming_text, "Hello");

        assert_eq!(app.on_event(END), [Committed::Agent("Hello".into())]);
        assert_eq!(app.streaming_text, "");
        assert!(!app.busy);
    }

    #[test]
    fn error_commits_the_partial_reply_then_the_error() {
        let mut app = App::default();
        app.on_event(delta("partial"));

        assert_eq!(
            app.on_event(Event::Error {
                message: "boom".into()
            }),
            [
                Committed::Agent("partial".into()),
                Committed::Error("boom".into())
            ]
        );
    }

    #[test]
    fn committed_reply_is_trimmed() {
        let mut app = App::default();
        app.on_event(delta("\n\nHello\n"));

        assert_eq!(app.on_event(END), [Committed::Agent("Hello".into())]);
    }

    #[test]
    fn tool_call_commits_the_text_before_it_then_the_call_and_result() {
        let mut app = App::default();
        app.on_event(delta("Let me look."));

        assert_eq!(
            app.on_event(Event::ToolCallStarted {
                name: "list_dir".into(),
                args: "{}".into()
            }),
            [
                Committed::Agent("Let me look.".into()),
                Committed::ToolCall {
                    name: "list_dir".into(),
                    args: "{}".into()
                }
            ]
        );
        assert_eq!(
            app.on_event(Event::ToolCallFinished {
                name: "list_dir".into(),
                status: ToolStatus::Completed,
                output: "src/\n".into()
            }),
            [Committed::ToolResult {
                name: "list_dir".into(),
                status: ToolStatus::Completed,
                output: "src/\n".into()
            }]
        );
    }

    #[test]
    fn turn_limit_is_explained() {
        let mut app = App {
            busy: true,
            ..App::default()
        };

        let committed = app.on_event(Event::TurnEnded {
            stop: StopReason::MaxTurnRequests,
        });

        assert!(matches!(&committed[..], [Committed::Error(e)] if e.contains("too many")));
        assert!(!app.busy);
    }

    #[test]
    fn turn_end_without_text_commits_nothing() {
        let mut app = App {
            busy: true,
            ..App::default()
        };

        assert!(app.on_event(END).is_empty());
        assert!(!app.busy);
    }
}
