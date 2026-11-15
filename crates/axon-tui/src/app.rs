//! UI state and how keys and session events change it. No terminal I/O.

use std::mem;
use std::time::{Duration, Instant};

use axon_protocol::{Event, StopReason, ToolStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default)]
pub(crate) struct App {
    pub(crate) input: String,
    /// The reply streaming in for the current turn.
    pub(crate) streaming_text: String,
    /// The running turn; Enter does nothing until it ends.
    pub(crate) turn: Option<Turn>,
}

/// What the activity row shows while a turn runs.
#[derive(Debug, PartialEq)]
pub(crate) struct Turn {
    started: Instant,
    /// Time since the prompt was sent, as of the last tick.
    pub(crate) elapsed: Duration,
    /// Advances on every tick; picks the spinner frame.
    pub(crate) frame: usize,
    pub(crate) activity: Activity,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Activity {
    Thinking,
    Running(String),
    /// Cancel was sent; the turn ends when the session confirms it.
    Cancelling,
}

impl Turn {
    fn new(started: Instant) -> Self {
        Self {
            started,
            elapsed: Duration::ZERO,
            frame: 0,
            activity: Activity::Thinking,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Submit(String),
    Cancel,
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
    Cancelled,
}

impl App {
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => Some(Action::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) if self.turn.is_none() => {
                Some(Action::Quit)
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => self.cancel(),
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
        if self.turn.is_some() || text.is_empty() {
            return None;
        }
        let text = text.to_string();
        self.input.clear();
        self.turn = Some(Turn::new(Instant::now()));
        Some(Action::Submit(text))
    }

    /// Ask to cancel the running turn, once.
    fn cancel(&mut self) -> Option<Action> {
        let turn = self.turn.as_mut()?;
        if turn.activity == Activity::Cancelling {
            return None;
        }
        turn.activity = Activity::Cancelling;
        Some(Action::Cancel)
    }

    /// Animate the activity row.
    pub(crate) fn on_tick(&mut self, now: Instant) {
        if let Some(turn) = &mut self.turn {
            turn.frame += 1;
            turn.elapsed = now.saturating_duration_since(turn.started);
        }
    }

    /// Show `activity` unless a cancel is already under way.
    fn set_activity(&mut self, activity: Activity) {
        if let Some(turn) = &mut self.turn
            && turn.activity != Activity::Cancelling
        {
            turn.activity = activity;
        }
    }

    pub(crate) fn on_event(&mut self, event: Event) -> Vec<Committed> {
        match event {
            Event::TextDelta { text } => {
                self.streaming_text.push_str(&text);
                Vec::new()
            }
            Event::ToolCallStarted { name, args } => {
                self.set_activity(Activity::Running(name.clone()));
                let mut committed = self.take_reply();
                committed.push(Committed::ToolCall { name, args });
                committed
            }
            Event::ToolCallFinished {
                name,
                status,
                output,
            } => {
                self.set_activity(Activity::Thinking);
                vec![Committed::ToolResult {
                    name,
                    status,
                    output,
                }]
            }
            Event::Error { message } => {
                let mut committed = self.take_reply();
                committed.push(Committed::Error(message));
                committed
            }
            Event::TurnEnded { stop } => {
                self.turn = None;
                let mut committed = self.take_reply();
                match stop {
                    StopReason::EndTurn => {}
                    StopReason::MaxTurnRequests => committed.push(Committed::Error(
                        "stopped: too many model requests for one prompt".into(),
                    )),
                    StopReason::Cancelled => committed.push(Committed::Cancelled),
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

    fn busy_app() -> App {
        App {
            turn: Some(Turn::new(Instant::now())),
            ..App::default()
        }
    }

    fn activity(app: &App) -> &Activity {
        &app.turn.as_ref().unwrap().activity
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
        assert!(app.turn.is_some());
    }

    #[test]
    fn enter_with_blank_input_does_nothing() {
        let mut app = App::default();
        type_text(&mut app, "  ");

        assert_eq!(app.on_key(key(KeyCode::Enter)), None);
        assert!(app.turn.is_none());
    }

    #[test]
    fn enter_while_busy_keeps_the_input() {
        let mut app = busy_app();
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
    fn ctrl_c_and_ctrl_q_quit_while_idle() {
        let mut app = App::default();

        assert_eq!(app.on_key(ctrl('c')), Some(Action::Quit));
        assert_eq!(app.on_key(ctrl('q')), Some(Action::Quit));
        assert_eq!(app.on_key(key(KeyCode::Esc)), None);
    }

    #[test]
    fn esc_or_ctrl_c_cancels_a_turn_once() {
        for cancel_key in [key(KeyCode::Esc), ctrl('c')] {
            let mut app = busy_app();

            assert_eq!(app.on_key(cancel_key), Some(Action::Cancel));
            assert_eq!(*activity(&app), Activity::Cancelling);
            assert_eq!(app.on_key(cancel_key), None);
        }
    }

    #[test]
    fn ctrl_q_quits_during_a_turn() {
        assert_eq!(busy_app().on_key(ctrl('q')), Some(Action::Quit));
    }

    #[test]
    fn activity_follows_tool_calls_but_cancelling_sticks() {
        let mut app = busy_app();
        let started = || Event::ToolCallStarted {
            name: "list_dir".into(),
            args: "{}".into(),
        };
        let finished = || Event::ToolCallFinished {
            name: "list_dir".into(),
            status: ToolStatus::Completed,
            output: String::new(),
        };

        app.on_event(started());
        assert_eq!(*activity(&app), Activity::Running("list_dir".into()));
        app.on_event(finished());
        assert_eq!(*activity(&app), Activity::Thinking);

        app.on_key(key(KeyCode::Esc));
        app.on_event(started());
        assert_eq!(*activity(&app), Activity::Cancelling);
    }

    #[test]
    fn ticks_advance_the_spinner_and_the_clock() {
        let mut app = busy_app();
        let started = app.turn.as_ref().unwrap().started;

        app.on_tick(started + Duration::from_secs(3));

        let turn = app.turn.as_ref().unwrap();
        assert_eq!((turn.frame, turn.elapsed), (1, Duration::from_secs(3)));
    }

    #[test]
    fn cancelled_turn_commits_the_partial_reply_then_cancelled() {
        let mut app = busy_app();
        app.on_event(delta("partial"));

        assert_eq!(
            app.on_event(Event::TurnEnded {
                stop: StopReason::Cancelled
            }),
            [Committed::Agent("partial".into()), Committed::Cancelled]
        );
        assert!(app.turn.is_none());
    }

    #[test]
    fn deltas_stream_then_turn_end_commits_the_reply() {
        let mut app = busy_app();

        assert!(app.on_event(delta("Hel")).is_empty());
        assert!(app.on_event(delta("lo")).is_empty());
        assert_eq!(app.streaming_text, "Hello");

        assert_eq!(app.on_event(END), [Committed::Agent("Hello".into())]);
        assert_eq!(app.streaming_text, "");
        assert!(app.turn.is_none());
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
        let mut app = busy_app();

        let committed = app.on_event(Event::TurnEnded {
            stop: StopReason::MaxTurnRequests,
        });

        assert!(matches!(&committed[..], [Committed::Error(e)] if e.contains("too many")));
        assert!(app.turn.is_none());
    }

    #[test]
    fn turn_end_without_text_commits_nothing() {
        let mut app = busy_app();

        assert!(app.on_event(END).is_empty());
        assert!(app.turn.is_none());
    }
}
