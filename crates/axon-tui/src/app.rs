//! UI state and how keys and session events change it. No terminal I/O.

use std::collections::HashMap;
use std::mem;
use std::time::{Duration, Instant};

use axon_protocol::{Change, Event, StopReason, ToolResult};
use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::prompt::Prompt;

/// History rows one wheel notch scrolls.
const WHEEL_ROWS: u32 = 3;

#[derive(Default)]
pub(crate) struct App {
    pub(crate) input: Prompt,
    /// The reply streaming in for the current turn.
    pub(crate) streaming_text: String,
    /// The model's reasoning streaming in, until its reply or a tool call starts.
    pub(crate) streaming_reasoning: String,
    /// A prompt sent while a turn ran: it goes out when the turn ends, or back
    /// into the input if the turn is cancelled.
    pub(crate) queued: Option<String>,
    /// Prompts sent this session, oldest first.
    pub(crate) past: Vec<String>,
    /// While Up and Down step through `past`: the entry shown, and the draft
    /// that was in the input before.
    pub(crate) recall: Option<(usize, Prompt)>,
    /// The model's context size in tokens, when the server reported it.
    pub(crate) context_limit: Option<u64>,
    /// The running turn; Enter does nothing until it ends.
    pub(crate) turn: Option<Turn>,
    /// How the last turn ended, shown in the activity row until the next one.
    pub(crate) finished: Option<Finished>,
    /// The id of a tool call waiting for y or n; its change is already in
    /// the history and the y/n prompt replaces the input.
    pub(crate) approval: Option<u64>,
    /// Names of the tool calls started and not yet finished, by id.
    pub(crate) calls: HashMap<u64, String>,
    /// The first history row shown, or `None` to follow the bottom.
    pub(crate) scroll: Option<u32>,
    /// The screen as last drawn.
    pub(crate) view: View,
}

/// Sizes from the last draw that keys need.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct View {
    /// The history pane's height in rows.
    pub(crate) height: u16,
    /// Rows the history takes.
    pub(crate) rows: u32,
    /// Columns the input's text wraps at.
    pub(crate) input_width: usize,
    /// The first input row shown.
    pub(crate) input_top: usize,
}

impl View {
    /// The first row shown when following the bottom.
    pub(crate) fn bottom(self) -> u32 {
        self.rows.saturating_sub(u32::from(self.height))
    }
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
    /// An error ended the turn.
    failed: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Finished {
    pub(crate) stop: StopReason,
    pub(crate) at: DateTime<Local>,
    pub(crate) took: Duration,
    pub(crate) failed: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Activity {
    Thinking,
    /// The model is streaming a call to this tool; `bytes` of arguments so far.
    Writing {
        name: String,
        bytes: usize,
    },
    Running(String),
    Approving,
    /// Waiting between model requests until the turn's `elapsed` reaches
    /// `until`; thinking after that.
    Cooling {
        until: Duration,
    },
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
            failed: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Submit(String),
    Answer { id: u64, approve: bool },
    Cancel,
    Quit,
}

/// A finished line for the history.
#[derive(Debug, PartialEq)]
pub(crate) enum Committed {
    User(String),
    Agent(String),
    /// The model's reasoning before a reply or tool call.
    Thought(String),
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        name: String,
        result: ToolResult,
    },
    /// A change asked for approval, kept whatever the answer.
    Change(Change),
    Error(String),
    Cancelled,
}

impl App {
    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::PageUp => return self.scroll_by(-i64::from(self.view.height)),
            KeyCode::PageDown => return self.scroll_by(i64::from(self.view.height)),
            KeyCode::End if key.modifiers == KeyModifiers::CONTROL => {
                return self.scroll_by(i64::from(self.view.rows));
            }
            _ => {}
        }
        if self.approval.is_some() {
            return self.on_approval_key(key);
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => Some(Action::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) if self.turn.is_none() => {
                Some(Action::Quit)
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => self.cancel(),
            // Terminals without the kitty keyboard protocol send Shift+Enter as
            // a line feed, which crossterm reports as Ctrl+J.
            (KeyCode::Enter, KeyModifiers::SHIFT) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.input.insert('\n');
                None
            }
            (KeyCode::Enter, _) => self.submit(),
            _ => {
                self.edit(key);
                None
            }
        }
    }

    /// Typing and moving the cursor in the input.
    fn edit(&mut self, key: KeyEvent) {
        let input = &mut self.input;
        match (key.code, key.modifiers) {
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => input.insert(c),
            (KeyCode::Backspace, _) => input.backspace(),
            (KeyCode::Delete, _) => input.delete(),
            // macOS terminals send Option+Left/Right as Alt+B/F.
            (KeyCode::Left, KeyModifiers::ALT | KeyModifiers::CONTROL)
            | (KeyCode::Char('b'), KeyModifiers::ALT) => input.word_left(),
            (KeyCode::Right, KeyModifiers::ALT | KeyModifiers::CONTROL)
            | (KeyCode::Char('f'), KeyModifiers::ALT) => input.word_right(),
            (KeyCode::Left, _) => input.left(),
            (KeyCode::Right, _) => input.right(),
            (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => input.home(),
            (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => input.end(),
            // On the first or last row, step through the prompts sent before.
            (KeyCode::Up, _) if !input.up(self.view.input_width) => self.recall_older(),
            (KeyCode::Down, _) if !input.down(self.view.input_width) => self.recall_newer(),
            _ => {}
        }
    }

    /// While a change waits, only y, n, cancel and quit do anything.
    fn on_approval_key(&mut self, key: KeyEvent) -> Option<Action> {
        let approve = match (key.code, key.modifiers) {
            (KeyCode::Char('y' | 'Y'), KeyModifiers::NONE | KeyModifiers::SHIFT) => true,
            (KeyCode::Char('n' | 'N'), KeyModifiers::NONE | KeyModifiers::SHIFT) => false,
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => return Some(Action::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                self.approval = None;
                return self.cancel();
            }
            _ => return None,
        };
        let id = self.approval.take()?;
        let activity = match self.calls.get(&id) {
            Some(name) if approve => Activity::Running(name.clone()),
            _ => Activity::Thinking,
        };
        self.set_activity(activity);
        Some(Action::Answer { id, approve })
    }

    /// Send the input, or queue it while a turn runs.
    fn submit(&mut self) -> Option<Action> {
        let text = self.input.text().trim();
        if text.is_empty() {
            return None;
        }
        let text = text.to_string();
        self.input.clear();
        self.recall = None;
        if self.past.last() != Some(&text) {
            self.past.push(text.clone());
        }
        if self.turn.is_some() {
            self.queued = Some(match self.queued.take() {
                Some(queued) => format!("{queued}\n{text}"),
                None => text,
            });
            return None;
        }
        self.turn = Some(Turn::new(Instant::now()));
        Some(Action::Submit(text))
    }

    /// Show the prompt sent before the one shown, keeping the draft.
    fn recall_older(&mut self) {
        let older = match &self.recall {
            Some((i, _)) => i.checked_sub(1),
            None => self.past.len().checked_sub(1),
        };
        let Some(i) = older else { return };
        match &mut self.recall {
            Some((shown, _)) => *shown = i,
            None => self.recall = Some((i, mem::take(&mut self.input))),
        }
        self.input = self.past[i].as_str().into();
    }

    /// Show the prompt sent after the one shown, or the draft after the newest.
    fn recall_newer(&mut self) {
        let Some((shown, _)) = &mut self.recall else {
            return;
        };
        if *shown + 1 < self.past.len() {
            *shown += 1;
            self.input = self.past[*shown].as_str().into();
        } else if let Some((_, draft)) = self.recall.take() {
            self.input = draft;
        }
    }

    /// Once the turn has ended, the queued prompt to send.
    pub(crate) fn send_queued(&mut self) -> Option<Action> {
        if self.turn.is_some() {
            return None;
        }
        let text = self.queued.take()?;
        self.turn = Some(Turn::new(Instant::now()));
        Some(Action::Submit(text))
    }

    pub(crate) fn on_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_by(-i64::from(WHEEL_ROWS)),
            MouseEventKind::ScrollDown => self.scroll_by(i64::from(WHEEL_ROWS)),
            _ => None,
        };
    }

    /// Move the history by `rows` (up when negative). Reaching the bottom
    /// follows it again, so new output shows; above it, the view stays put.
    fn scroll_by(&mut self, rows: i64) -> Option<Action> {
        let bottom = i64::from(self.view.bottom());
        let top = i64::from(self.scroll.unwrap_or(self.view.bottom())) + rows;
        self.scroll = (top < bottom).then(|| top.max(0) as u32);
        None
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
            Event::ReasoningDelta { text } => {
                self.streaming_reasoning.push_str(&text);
                Vec::new()
            }
            Event::TextDelta { text } => {
                self.streaming_text.push_str(&text);
                self.take_thought().into_iter().collect()
            }
            Event::ToolCallStreaming { name, bytes } => {
                self.set_activity(Activity::Writing { name, bytes });
                self.take_thought().into_iter().collect()
            }
            Event::ToolCallStarted { id, name, args } => {
                self.set_activity(Activity::Running(name.clone()));
                self.calls.insert(id, name.clone());
                let mut committed = self.take_reply();
                committed.push(Committed::ToolCall { name, args });
                committed
            }
            Event::ApprovalRequested { id, change } => {
                self.set_activity(Activity::Approving);
                self.approval = Some(id);
                let mut committed = self.take_reply();
                committed.push(Committed::Change(change));
                committed
            }
            Event::ToolCallFinished { id, result } => {
                self.set_activity(Activity::Thinking);
                self.approval = None;
                let name = self.calls.remove(&id).unwrap_or_default();
                vec![Committed::ToolResult { name, result }]
            }
            Event::Paused { secs } => {
                if let Some(turn) = &self.turn {
                    let until = turn.started.elapsed() + Duration::from_secs(secs);
                    self.set_activity(Activity::Cooling { until });
                }
                Vec::new()
            }
            Event::Error { message } => {
                if let Some(turn) = &mut self.turn {
                    turn.failed = true;
                }
                let mut committed = self.take_reply();
                committed.push(Committed::Error(message));
                committed
            }
            Event::ContextLimit { tokens } => {
                self.context_limit = tokens;
                Vec::new()
            }
            Event::TurnEnded { stop } => {
                let cancelling = self
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.activity == Activity::Cancelling);
                if let Some(turn) = self.turn.take() {
                    self.finished = Some(Finished {
                        stop,
                        at: Local::now(),
                        took: turn.started.elapsed(),
                        failed: turn.failed,
                    });
                }
                self.approval = None;
                self.calls.clear();
                let mut committed = self.take_reply();
                match stop {
                    StopReason::EndTurn => {}
                    StopReason::MaxTurnRequests => committed.push(Committed::Error(
                        "stopped: too many model requests for one prompt".into(),
                    )),
                    StopReason::Cancelled => committed.push(Committed::Cancelled),
                    StopReason::MaxTokens => committed.push(Committed::Error(
                        "stopped: the reply reached the token limit (AXON_MAX_TOKENS)".into(),
                    )),
                }
                if (stop == StopReason::Cancelled || cancelling)
                    && let Some(queued) = self.queued.take()
                {
                    self.input = match self.input.text() {
                        "" => queued.as_str().into(),
                        typed => format!("{queued}\n{typed}").as_str().into(),
                    };
                }
                committed
            }
        }
    }

    /// The reasoning so far, then the reply so far.
    fn take_reply(&mut self) -> Vec<Committed> {
        let mut committed: Vec<_> = self.take_thought().into_iter().collect();
        let text = mem::take(&mut self.streaming_text);
        let text = text.trim();
        if !text.is_empty() {
            committed.push(Committed::Agent(text.into()));
        }
        committed
    }

    fn take_thought(&mut self) -> Option<Committed> {
        let text = mem::take(&mut self.streaming_reasoning);
        let text = text.trim();
        (!text.is_empty()).then(|| Committed::Thought(text.into()))
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
        assert_eq!(app.input.text(), "");
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
    fn shift_enter_and_ctrl_j_insert_a_newline() {
        let mut app = App::default();
        type_text(&mut app, "a");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut app, "b");
        app.on_key(ctrl('j'));

        assert_eq!(app.input.text(), "a\nb\n");
    }

    #[test]
    fn backspace_removes_the_last_char() {
        let mut app = App::default();
        type_text(&mut app, "hé");
        app.on_key(key(KeyCode::Backspace));

        assert_eq!(app.input.text(), "h");
    }

    #[test]
    fn shift_types_and_other_modifiers_do_not() {
        let mut app = App::default();
        app.on_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        app.on_key(ctrl('x'));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));

        assert_eq!(app.input.text(), "A");
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
            id: 1,
            name: "list_dir".into(),
            args: "{}".into(),
        };
        let finished = || Event::ToolCallFinished {
            id: 1,
            result: ToolResult::Output(String::new()),
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
    fn a_turn_that_errored_finishes_as_failed() {
        let mut app = busy_app();
        app.on_event(Event::Error {
            message: "boom".into(),
        });
        app.on_event(END);

        assert!(app.finished.unwrap().failed);
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
                id: 1,
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
                id: 1,
                result: ToolResult::Output("src/\n".into())
            }),
            [Committed::ToolResult {
                name: "list_dir".into(),
                result: ToolResult::Output("src/\n".into())
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
    fn reasoning_streams_then_commits_when_the_reply_starts() {
        let mut app = busy_app();

        assert!(
            app.on_event(Event::ReasoningDelta { text: "Hmm".into() })
                .is_empty()
        );
        assert_eq!(app.streaming_reasoning, "Hmm");

        let committed = app.on_event(Event::TextDelta { text: "391".into() });

        assert_eq!(committed, [Committed::Thought("Hmm".into())]);
        assert!(app.streaming_reasoning.is_empty());
        assert_eq!(app.streaming_text, "391");
    }

    #[test]
    fn a_streaming_tool_call_shows_its_size_and_commits_the_reasoning() {
        let mut app = busy_app();
        app.on_event(Event::ReasoningDelta {
            text: "I'll edit it.".into(),
        });

        let committed = app.on_event(Event::ToolCallStreaming {
            name: "edit_file".into(),
            bytes: 3200,
        });

        assert_eq!(committed, [Committed::Thought("I'll edit it.".into())]);
        assert_eq!(
            app.turn.unwrap().activity,
            Activity::Writing {
                name: "edit_file".into(),
                bytes: 3200
            }
        );
    }

    #[test]
    fn enter_during_a_turn_queues_the_prompt_until_the_turn_ends() {
        let mut app = busy_app();
        type_text(&mut app, "next");

        assert_eq!(app.on_key(key(KeyCode::Enter)), None);
        assert_eq!(app.queued.as_deref(), Some("next"));
        assert_eq!(app.input.text(), "");
        assert_eq!(app.send_queued(), None, "the turn still runs");

        app.on_event(Event::TurnEnded {
            stop: StopReason::EndTurn,
        });

        assert_eq!(app.send_queued(), Some(Action::Submit("next".into())));
        assert!(app.turn.is_some());
        assert_eq!(app.queued, None);
    }

    #[test]
    fn a_cancelled_turn_puts_the_queued_prompt_back_in_the_input() {
        let mut app = busy_app();
        type_text(&mut app, "first");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "second");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "typing");

        app.on_event(Event::TurnEnded {
            stop: StopReason::Cancelled,
        });

        assert_eq!(app.input.text(), "first\nsecond\ntyping");
        assert_eq!(app.send_queued(), None);
    }

    #[test]
    fn a_turn_that_ends_while_cancelling_keeps_the_queued_prompt_in_the_input() {
        let mut app = busy_app();
        type_text(&mut app, "next");
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Esc));

        app.on_event(Event::TurnEnded {
            stop: StopReason::EndTurn,
        });

        assert_eq!(app.send_queued(), None);
        assert_eq!(app.input.text(), "next");
    }

    #[test]
    fn token_limit_is_explained() {
        let mut app = busy_app();

        let committed = app.on_event(Event::TurnEnded {
            stop: StopReason::MaxTokens,
        });

        assert!(matches!(&committed[..], [Committed::Error(e)] if e.contains("AXON_MAX_TOKENS")));
    }

    #[test]
    fn turn_end_records_how_it_ended() {
        let mut app = busy_app();

        app.on_event(Event::TurnEnded {
            stop: StopReason::Cancelled,
        });

        assert_eq!(app.finished.map(|f| f.stop), Some(StopReason::Cancelled));
    }

    #[test]
    fn a_pause_cools_down_until_its_end() {
        let mut app = busy_app();

        assert!(app.on_event(Event::Paused { secs: 30 }).is_empty());
        let Activity::Cooling { until } = *activity(&app) else {
            panic!("{:?}", activity(&app));
        };
        assert!(until >= Duration::from_secs(30) && until < Duration::from_secs(31));
    }

    #[test]
    fn turn_end_without_text_commits_nothing() {
        let mut app = busy_app();

        assert!(app.on_event(END).is_empty());
        assert!(app.turn.is_none());
    }

    fn change() -> Change {
        Change::WriteFile {
            path: "a.txt".into(),
            old_text: None,
            new_text: "hi\n".into(),
        }
    }

    fn asked_app() -> App {
        let mut app = busy_app();
        app.on_event(Event::ToolCallStarted {
            id: 7,
            name: "write_file".into(),
            args: "{}".into(),
        });
        app.on_event(Event::ApprovalRequested {
            id: 7,
            change: change(),
        });
        app
    }

    fn finished(result: ToolResult) -> Event {
        Event::ToolCallFinished { id: 7, result }
    }

    #[test]
    fn approval_request_commits_the_reply_and_waits() {
        let mut app = busy_app();
        app.on_event(delta("I'll write it."));

        let committed = app.on_event(Event::ApprovalRequested {
            id: 7,
            change: change(),
        });

        assert_eq!(
            committed,
            [
                Committed::Agent("I'll write it.".into()),
                Committed::Change(change())
            ]
        );
        assert_eq!(*activity(&app), Activity::Approving);
        assert_eq!(app.approval, Some(7));
    }

    #[test]
    fn y_accepts_runs_and_the_result_follows_the_change() {
        let mut app = asked_app();

        assert_eq!(
            app.on_key(key(KeyCode::Char('y'))),
            Some(Action::Answer {
                id: 7,
                approve: true
            })
        );
        assert!(app.approval.is_none());
        assert_eq!(*activity(&app), Activity::Running("write_file".into()));
        assert_eq!(
            app.on_event(finished(ToolResult::Output("out".into()))),
            [Committed::ToolResult {
                name: "write_file".into(),
                result: ToolResult::Output("out".into())
            }]
        );
    }

    #[test]
    fn n_rejects_and_the_rejection_follows_the_change() {
        let mut app = asked_app();

        assert_eq!(
            app.on_key(key(KeyCode::Char('n'))),
            Some(Action::Answer {
                id: 7,
                approve: false
            })
        );
        assert_eq!(*activity(&app), Activity::Thinking);
        assert_eq!(
            app.on_event(finished(ToolResult::Rejected)),
            [Committed::ToolResult {
                name: "write_file".into(),
                result: ToolResult::Rejected
            }]
        );
    }

    #[test]
    fn other_keys_are_ignored_while_asked() {
        let mut app = asked_app();

        assert_eq!(app.on_key(key(KeyCode::Char('x'))), None);
        assert_eq!(app.on_key(key(KeyCode::Enter)), None);
        assert_eq!(app.on_key(ctrl('y')), None);
        assert_eq!(app.input.text(), "");
        assert!(app.approval.is_some());
    }

    #[test]
    fn esc_while_asked_cancels_the_turn() {
        let mut app = asked_app();

        assert_eq!(app.on_key(key(KeyCode::Esc)), Some(Action::Cancel));
        assert!(app.approval.is_none());
        assert_eq!(*activity(&app), Activity::Cancelling);
    }

    fn scrolled_app() -> App {
        App {
            view: View {
                height: 10,
                rows: 50,
                input_width: 20,
                input_top: 0,
            },
            ..App::default()
        }
    }

    fn wheel(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn wheel_scrolls_up_from_the_bottom_and_back_to_following_it() {
        let mut app = scrolled_app();

        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.scroll, Some(40 - WHEEL_ROWS));

        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.scroll, None);
    }

    #[test]
    fn page_keys_move_a_pane_and_stop_at_the_top() {
        let mut app = scrolled_app();

        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.scroll, Some(30));
        for _ in 0..5 {
            app.on_key(key(KeyCode::PageUp));
        }
        assert_eq!(app.scroll, Some(0));

        app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(app.scroll, None);
    }

    #[test]
    fn end_moves_in_the_input_and_leaves_the_history() {
        let mut app = scrolled_app();
        type_text(&mut app, "ab");
        app.on_key(key(KeyCode::PageUp));
        app.on_key(key(KeyCode::Home));

        app.on_key(key(KeyCode::End));
        type_text(&mut app, "c");

        assert_eq!(app.input.text(), "abc");
        assert_eq!(app.scroll, Some(30));
    }

    /// Send `text` as a prompt and end its turn.
    fn send(app: &mut App, text: &str) {
        type_text(app, text);
        app.on_key(key(KeyCode::Enter));
        app.turn = None;
    }

    fn history_app() -> App {
        let mut app = App::default();
        app.view.input_width = 40;
        app
    }

    #[test]
    fn up_and_down_step_through_sent_prompts_then_back_to_the_draft() {
        let mut app = history_app();
        send(&mut app, "one");
        send(&mut app, "two");
        type_text(&mut app, "draft");

        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "two");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "one");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "one");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text(), "two");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text(), "draft");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text(), "draft");
    }

    #[test]
    fn up_moves_through_a_recalled_prompt_before_the_one_before_it() {
        let mut app = history_app();
        send(&mut app, "one");
        app.input = "two\nlines".into();
        app.on_key(key(KeyCode::Enter));
        app.turn = None;

        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "two\nlines");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "two\nlines");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "one");
    }

    #[test]
    fn a_prompt_sent_twice_in_a_row_is_recalled_once() {
        let mut app = history_app();
        send(&mut app, "one");
        send(&mut app, "two");
        send(&mut app, "two");

        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "one");
    }

    #[test]
    fn a_queued_prompt_is_recalled_too() {
        let mut app = history_app();
        app.turn = Some(Turn::new(Instant::now()));
        type_text(&mut app, "later");
        app.on_key(key(KeyCode::Enter));

        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "later");
    }

    #[test]
    fn keys_edit_at_the_cursor() {
        let mut app = scrolled_app();
        type_text(&mut app, "one two");
        app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        app.on_key(key(KeyCode::Backspace));
        app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        app.on_key(key(KeyCode::Delete));
        app.on_key(ctrl('e'));
        app.on_key(key(KeyCode::Left));
        type_text(&mut app, "!");
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        app.on_key(key(KeyCode::Up));
        app.on_key(ctrl('a'));
        type_text(&mut app, ">");

        assert_eq!(app.input.text(), ">netw!\no");
    }

    #[test]
    fn scrolling_works_while_asked() {
        let mut app = asked_app();
        app.view = scrolled_app().view;

        app.on_key(key(KeyCode::PageUp));

        assert_eq!(app.scroll, Some(30));
        assert_eq!(app.approval, Some(7));
    }

    #[test]
    fn the_context_limit_is_kept_for_the_session() {
        let mut app = App::default();

        app.on_event(Event::ContextLimit {
            tokens: Some(262144),
        });

        assert_eq!(app.context_limit, Some(262144));
    }
}
