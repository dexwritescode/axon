//! Saved sessions: one JSON-lines file per session, each line an entry
//! linked to the one before. `prompt` and `event` entries are what the
//! frontend showed; `message` entries are the model history.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Event, Past, Resume};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Upgrades an entry, as JSON, from the format before a version.
type Migration = (&'static str, fn(Value) -> Value);

/// Format changes, oldest first: each upgrades entries written before its
/// version. Versions that changed nothing have none.
const MIGRATIONS: &[Migration] = &[];

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    id: Uuid,
    parent: Option<Uuid>,
    /// The axon version that wrote it.
    version: String,
    #[serde(flatten)]
    kind: Kind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Kind {
    Header {
        cwd: PathBuf,
        model: String,
    },
    Prompt {
        text: String,
    },
    Event {
        event: Event,
    },
    Message {
        message: ChatCompletionRequestMessage,
    },
}

/// One turn, kept until it ends and then written at once.
#[derive(Debug, Default)]
pub(crate) struct Turn {
    prompt: String,
    events: Vec<Event>,
    pub(crate) messages: Vec<ChatCompletionRequestMessage>,
}

impl Turn {
    pub(crate) fn new(prompt: String) -> Self {
        Self {
            prompt,
            ..Self::default()
        }
    }

    /// Keep an event to redraw, joining deltas; progress events aren't kept.
    pub(crate) fn record(&mut self, event: &Event) {
        match (self.events.last_mut(), event) {
            (Some(Event::TextDelta { text }), Event::TextDelta { text: more })
            | (Some(Event::ReasoningDelta { text }), Event::ReasoningDelta { text: more }) => {
                text.push_str(more)
            }
            (_, Event::ToolCallStreaming { .. } | Event::Paused { .. }) => {}
            (_, Event::ContextLimit { .. }) => {}
            _ => self.events.push(event.clone()),
        }
    }
}

/// What a saved session holds.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Saved {
    pub(crate) past: Vec<Past>,
    pub(crate) history: Vec<ChatCompletionRequestMessage>,
}

/// Where one session is saved.
#[derive(Debug)]
pub(crate) struct Store {
    id: Uuid,
    path: PathBuf,
    cwd: PathBuf,
    model: String,
    /// The last entry written, the parent of the next.
    last: Option<Uuid>,
}

impl Store {
    /// A new session under `root`; its file is made by the first turn.
    pub(crate) fn new(root: &Path, cwd: &Path, model: &str) -> Self {
        let id = Uuid::now_v7();
        Self {
            id,
            path: dir(root, cwd).join(format!("{id}.jsonl")),
            cwd: cwd.into(),
            model: model.into(),
            last: None,
        }
    }

    /// The saved session `resume` names, or None if there isn't one.
    pub(crate) fn open(
        root: &Path,
        cwd: &Path,
        model: &str,
        resume: &Resume,
    ) -> Result<Option<(Self, Saved)>> {
        let ids = saved_ids(&dir(root, cwd))?;
        let id = match resume {
            Resume::New => return Ok(Some((Self::new(root, cwd, model), Saved::default()))),
            Resume::Latest => ids.into_iter().max(),
            Resume::Id(prefix) => {
                let mut matching = ids
                    .into_iter()
                    .filter(|id| id.to_string().starts_with(prefix.as_str()));
                let first = matching.next();
                if first.is_some() && matching.next().is_some() {
                    bail!("more than one session starts with {prefix}");
                }
                first
            }
        };
        let Some(id) = id else { return Ok(None) };
        let path = dir(root, cwd).join(format!("{id}.jsonl"));
        let (saved, last) = read(&path)?;
        let store = Self {
            id,
            path,
            cwd: cwd.into(),
            model: model.into(),
            last,
        };
        Ok(Some((store, saved)))
    }

    pub(crate) fn id(&self) -> String {
        self.id.to_string()
    }

    /// Append a turn, starting the file with a header if it's new.
    pub(crate) fn append(&mut self, turn: Turn) -> Result<()> {
        let mut kinds = Vec::new();
        if self.last.is_none() {
            kinds.push(Kind::Header {
                cwd: self.cwd.clone(),
                model: self.model.clone(),
            });
        }
        kinds.push(Kind::Prompt { text: turn.prompt });
        kinds.extend(turn.events.into_iter().map(|event| Kind::Event { event }));
        kinds.extend(
            turn.messages
                .into_iter()
                .map(|message| Kind::Message { message }),
        );
        let mut lines = String::new();
        for kind in kinds {
            let id = Uuid::now_v7();
            let entry = Entry {
                id,
                parent: self.last,
                version: env!("CARGO_PKG_VERSION").into(),
                kind,
            };
            lines.push_str(&serde_json::to_string(&entry)?);
            lines.push('\n');
            self.last = Some(id);
        }
        let parent = self.path.parent().expect("a session file has a folder");
        fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(lines.as_bytes()))
            .with_context(|| self.path.display().to_string())
    }
}

/// The folder for sessions started in `cwd`: its path with every other
/// character than letters and digits made a `-`.
fn dir(root: &Path, cwd: &Path) -> PathBuf {
    let key: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    root.join(key)
}

fn saved_ids(dir: &Path) -> Result<Vec<Uuid>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| dir.display().to_string()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "jsonl")
            && let Some(id) = path.file_stem().and_then(|s| s.to_str()?.parse().ok())
        {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Read a session file and the id of its last entry. Entries a newer axon
/// wrote are skipped, unless they are messages: the history needs them all.
fn read(path: &Path) -> Result<(Saved, Option<Uuid>)> {
    let text = fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let mut saved = Saved::default();
    let mut last = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let value = migrate(value, MIGRATIONS);
        if let Some(id) = value["id"].as_str().and_then(|id| id.parse().ok()) {
            last = Some(id);
        }
        match serde_json::from_value::<Entry>(value.clone()) {
            Ok(entry) => match entry.kind {
                Kind::Header { .. } => {}
                Kind::Prompt { text } => saved.past.push(Past::Prompt(text)),
                Kind::Event { event } => saved.past.push(Past::Event(event)),
                Kind::Message { message } => saved.history.push(message),
            },
            Err(_) if value["type"] == "message" => bail!(
                "{}: a message written by a newer axon ({})",
                path.display(),
                value["version"]
            ),
            Err(_) => {}
        }
    }
    Ok((saved, last))
}

/// Run each migration whose version is after the one that wrote `entry`,
/// ignoring `-dev`: a dev build already writes its release's format.
fn migrate(mut entry: Value, migrations: &[Migration]) -> Value {
    let Some(written) = entry["version"].as_str().and_then(release) else {
        return entry;
    };
    for (at, upgrade) in migrations {
        if written < release(at).expect("a migration's version is valid") {
            entry = upgrade(entry);
        }
    }
    entry
}

fn release(version: &str) -> Option<Version> {
    let v = Version::parse(version).ok()?;
    Some(Version::new(v.major, v.minor, v.patch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::{Change, Exit, StopReason, ToolResult};
    use serde_json::json;

    const GOLDEN_0_1_1: &str = include_str!("../tests/sessions/0.1.1.jsonl");

    fn message(value: Value) -> ChatCompletionRequestMessage {
        serde_json::from_value(value).unwrap()
    }

    fn started(id: u64, name: &str) -> Event {
        Event::ToolCallStarted {
            id,
            name: name.into(),
            args: "{}".into(),
        }
    }

    fn finished(id: u64, result: ToolResult) -> Event {
        Event::ToolCallFinished { id, result }
    }

    fn command(exit: Exit) -> ToolResult {
        ToolResult::Command {
            head: "out".into(),
            cut: 0,
            tail: String::new(),
            exit,
        }
    }

    /// Turns that use every event, change and tool result.
    fn turns() -> Vec<(&'static str, Vec<Event>, Vec<Value>)> {
        let text = |t: &str| Event::TextDelta { text: t.into() };
        let reasoning = |t: &str| Event::ReasoningDelta { text: t.into() };
        let end = |stop| Event::TurnEnded { stop };
        vec![
            (
                "write a.txt",
                vec![
                    reasoning("Plan"),
                    reasoning(" it."),
                    text("I'll"),
                    text(" write it."),
                    Event::ToolCallStreaming {
                        name: "write_file".into(),
                        bytes: 9,
                    },
                    started(1, "write_file"),
                    Event::ApprovalRequested {
                        id: 1,
                        change: Change::WriteFile {
                            path: "a.txt".into(),
                            old_text: None,
                            new_text: "hi\n".into(),
                        },
                    },
                    finished(1, ToolResult::Output("wrote a.txt".into())),
                    Event::Paused { secs: 30 },
                    text("Done."),
                    end(StopReason::EndTurn),
                ],
                vec![
                    json!({"role": "user", "content": "write a.txt"}),
                    json!({"role": "assistant", "content": "I'll write it.", "tool_calls": [
                        {"id": "call_1", "type": "function",
                         "function": {"name": "write_file", "arguments": "{}"}}
                    ]}),
                    json!({"role": "tool", "content": "wrote a.txt", "tool_call_id": "call_1"}),
                    json!({"role": "assistant", "content": "Done."}),
                ],
            ),
            (
                "run things",
                vec![
                    started(2, "run_command"),
                    Event::ApprovalRequested {
                        id: 2,
                        change: Change::RunCommand {
                            command: "ls".into(),
                        },
                    },
                    finished(2, command(Exit::Code(1))),
                    finished(3, command(Exit::Killed)),
                    finished(4, command(Exit::TimedOut { secs: 2 })),
                    finished(5, command(Exit::Cancelled)),
                    finished(6, ToolResult::Failed("no such file".into())),
                    finished(7, ToolResult::Rejected),
                    end(StopReason::MaxTurnRequests),
                ],
                vec![json!({"role": "user", "content": "run things"})],
            ),
            (
                "fail",
                vec![
                    Event::Error {
                        message: "boom".into(),
                    },
                    end(StopReason::EndTurn),
                ],
                vec![],
            ),
            (
                "stop",
                vec![end(StopReason::Cancelled)],
                vec![
                    json!({"role": "user", "content": "stop"}),
                    json!({"role": "assistant", "content": ""}),
                ],
            ),
            (
                "long",
                vec![text("Once"), end(StopReason::MaxTokens)],
                vec![
                    json!({"role": "user", "content": "long"}),
                    json!({"role": "assistant", "content": "Once"}),
                ],
            ),
        ]
    }

    fn write_turns(store: &mut Store) {
        for (prompt, events, messages) in turns() {
            let mut turn = Turn::new(prompt.into());
            for event in &events {
                turn.record(event);
            }
            turn.messages = messages.into_iter().map(message).collect();
            store.append(turn).unwrap();
        }
    }

    /// The lines of a session with what changes per run (ids, version) blanked.
    fn masked(text: &str) -> Vec<Value> {
        text.lines()
            .map(|line| {
                let mut value: Value = serde_json::from_str(line).unwrap();
                for field in ["id", "parent", "version"] {
                    if !value[field].is_null() {
                        value[field] = json!("…");
                    }
                }
                value
            })
            .collect()
    }

    fn new_store(root: &Path) -> Store {
        Store::new(root, Path::new("/work/app"), "test-model")
    }

    #[test]
    fn a_session_is_written_as_the_golden_file() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        write_turns(&mut store);

        let written = fs::read_to_string(&store.path).unwrap();
        assert_eq!(masked(&written), masked(GOLDEN_0_1_1));
        assert_eq!(
            store.path,
            root.path()
                .join("-work-app")
                .join(format!("{}.jsonl", store.id))
        );
    }

    #[test]
    fn each_entry_names_the_one_before_as_its_parent() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        write_turns(&mut store);

        let entries: Vec<Value> = fs::read_to_string(&store.path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(entries[0]["parent"], Value::Null);
        for pair in entries.windows(2) {
            assert_eq!(pair[1]["parent"], pair[0]["id"]);
        }
        assert_eq!(entries[0]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_golden_file_reads_back_as_what_was_shown_and_the_history() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sessions/0.1.1.jsonl");
        let (saved, last) = read(&path).unwrap();

        let mut past = Vec::new();
        let mut history = Vec::new();
        for (prompt, events, messages) in turns() {
            let mut turn = Turn::new(prompt.into());
            events.iter().for_each(|event| turn.record(event));
            past.push(Past::Prompt(prompt.into()));
            past.extend(turn.events.into_iter().map(Past::Event));
            history.extend(messages.into_iter().map(message));
        }
        assert_eq!(saved, Saved { past, history });
        let last_line: Value = serde_json::from_str(GOLDEN_0_1_1.lines().last().unwrap()).unwrap();
        assert_eq!(
            last.map(|id| id.to_string()),
            last_line["id"].as_str().map(String::from)
        );
    }

    #[test]
    fn record_joins_deltas_and_drops_progress() {
        let mut turn = Turn::new("hi".into());
        for event in [
            Event::ReasoningDelta { text: "a".into() },
            Event::ReasoningDelta { text: "b".into() },
            Event::Paused { secs: 1 },
            Event::TextDelta { text: "c".into() },
            Event::ToolCallStreaming {
                name: "x".into(),
                bytes: 1,
            },
            Event::TextDelta { text: "d".into() },
        ] {
            turn.record(&event);
        }

        assert_eq!(
            turn.events,
            [
                Event::ReasoningDelta { text: "ab".into() },
                Event::TextDelta { text: "cd".into() }
            ]
        );
    }

    #[test]
    fn a_reopened_session_continues_the_chain_without_a_second_header() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        store.append(Turn::new("one".into())).unwrap();
        let last = store.last;

        let (mut store, saved) = Store::open(
            root.path(),
            Path::new("/work/app"),
            "test-model",
            &Resume::Latest,
        )
        .unwrap()
        .unwrap();
        assert_eq!(store.last, last);
        assert_eq!(saved.past, [Past::Prompt("one".into())]);
        store.append(Turn::new("two".into())).unwrap();

        let text = fs::read_to_string(&store.path).unwrap();
        let types: Vec<_> = masked(&text).iter().map(|v| v["type"].clone()).collect();
        assert_eq!(types, ["header", "prompt", "prompt"]);
    }

    #[test]
    fn open_finds_the_newest_or_an_id_prefix_in_the_same_folder_only() {
        let root = tempfile::tempdir().unwrap();
        let open = |cwd: &str, resume| {
            Store::open(root.path(), Path::new(cwd), "m", &resume)
                .map(|found| found.map(|(store, _)| store.id))
        };
        let mut ids = Vec::new();
        for prompt in ["first", "second"] {
            let mut store = new_store(root.path());
            store.append(Turn::new(prompt.into())).unwrap();
            ids.push(store.id);
        }

        assert_eq!(open("/work/app", Resume::Latest).unwrap(), Some(ids[1]));
        let prefix = ids[0].to_string()[..30].to_string();
        assert_eq!(open("/work/app", Resume::Id(prefix)).unwrap(), Some(ids[0]));
        assert_eq!(open("/work/app", Resume::Id("nope".into())).unwrap(), None);
        assert!(open("/work/app", Resume::Id(String::new())).is_err());
        assert_eq!(open("/work/other", Resume::Latest).unwrap(), None);
        let new = open("/work/other", Resume::New).unwrap().unwrap();
        assert!(!ids.contains(&new));
    }

    #[test]
    fn entries_a_newer_axon_wrote_are_skipped_unless_they_are_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let entry = |kind: Value| {
            let mut value = json!({"id": Uuid::now_v7(), "parent": null, "version": "9.0.0"});
            value
                .as_object_mut()
                .unwrap()
                .extend(kind.as_object().unwrap().clone());
            value.to_string()
        };
        let lines = [
            entry(json!({"type": "prompt", "text": "hi"})),
            entry(json!({"type": "event", "event": {"type": "something_new"}})),
            entry(json!({"type": "fancy"})),
            "{not json".into(),
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        let (saved, _) = read(&path).unwrap();
        assert_eq!(saved.past, [Past::Prompt("hi".into())]);

        let message = entry(json!({"type": "message", "message": {"role": "wizard"}}));
        fs::write(&path, [lines[0].clone(), message].join("\n")).unwrap();
        let err = read(&path).unwrap_err().to_string();
        assert!(err.contains("newer axon") && err.contains("9.0.0"), "{err}");
    }

    #[test]
    fn migrations_run_in_order_for_each_version_after_the_writer() {
        fn add(step: &'static str) -> impl Fn(Value) -> Value {
            move |mut v| {
                v["steps"].as_array_mut().unwrap().push(json!(step));
                v
            }
        }
        const MIGRATIONS: &[Migration] = &[
            ("0.2.0", |v| add("0.2.0")(v)),
            ("0.4.0", |v| add("0.4.0")(v)),
        ];
        let steps = |version: &str| {
            migrate(json!({"version": version, "steps": []}), MIGRATIONS)["steps"].clone()
        };

        assert_eq!(steps("0.1.5"), json!(["0.2.0", "0.4.0"]));
        assert_eq!(steps("0.2.0"), json!(["0.4.0"]));
        assert_eq!(steps("0.3.0"), json!(["0.4.0"]));
        assert_eq!(steps("0.4.0-dev"), json!([]));
        assert_eq!(steps("0.4.1"), json!([]));
    }
}
