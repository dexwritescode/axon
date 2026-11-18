//! Saved sessions: one JSON-lines file per session, each line an entry
//! linked to the one before. `prompt` and `event` entries are what the
//! frontend showed; `message` entries are the model history.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, ErrorKind, Read as _, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    /// The file, locked so no other axon writes to it; opened by the first
    /// turn of a new session.
    file: Option<File>,
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
            file: None,
        }
    }

    /// The saved session `resume` names, or None if there isn't one. A last
    /// turn left incomplete, by a crash mid-write, is cut from the file.
    pub(crate) fn open(
        root: &Path,
        cwd: &Path,
        model: &str,
        resume: &Resume,
    ) -> Result<Option<(Self, Saved)>> {
        let found = match resume {
            Resume::New => return Ok(Some((Self::new(root, cwd, model), Saved::default()))),
            Resume::Latest => saved(&dir(root, cwd), cwd)?
                .into_iter()
                .max_by_key(|&(id, used)| (used, id)),
            Resume::Id(prefix) => {
                let prefix = prefix.to_lowercase();
                if prefix.is_empty() {
                    bail!("a session id can't be empty");
                }
                let mut matching = saved(&dir(root, cwd), cwd)?
                    .into_iter()
                    .filter(|(id, _)| id.to_string().starts_with(&prefix));
                let first = matching.next();
                if first.is_some() && matching.next().is_some() {
                    bail!("more than one session starts with {prefix}");
                }
                first
            }
        };
        let Some((id, _)) = found else {
            return Ok(None);
        };
        let path = dir(root, cwd).join(format!("{id}.jsonl"));
        let mut file = lock(&path)?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .with_context(|| path.display().to_string())?;
        let read = read(&text, &path)?;
        let repaired = match read.complete {
            Some(len) => file.set_len(len),
            None if !read.ends_in_newline => file.write_all(b"\n"),
            None => Ok(()),
        };
        repaired.with_context(|| path.display().to_string())?;
        let store = Self {
            id,
            path,
            cwd: cwd.into(),
            model: model.into(),
            last: read.last,
            file: Some(file),
        };
        Ok(Some((store, read.saved)))
    }

    pub(crate) fn id(&self) -> String {
        self.id.to_string()
    }

    /// Append a turn, starting the file with a header if it's new. Its
    /// messages come before its events, so `TurnEnded` closes it.
    pub(crate) fn append(&mut self, turn: Turn) -> Result<()> {
        let mut kinds = Vec::new();
        if self.last.is_none() {
            kinds.push(Kind::Header {
                cwd: self.cwd.clone(),
                model: self.model.clone(),
            });
        }
        kinds.push(Kind::Prompt { text: turn.prompt });
        kinds.extend(
            turn.messages
                .into_iter()
                .map(|message| Kind::Message { message }),
        );
        kinds.extend(turn.events.into_iter().map(|event| Kind::Event { event }));
        let mut lines = String::new();
        for kind in kinds {
            let id = Uuid::now_v7();
            let entry = Entry {
                id,
                parent: self.last,
                version: VERSION.into(),
                kind,
            };
            lines.push_str(&serde_json::to_string(&entry)?);
            lines.push('\n');
            self.last = Some(id);
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => {
                let parent = self.path.parent().expect("a session file has a folder");
                fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
                self.file.insert(lock(&self.path)?)
            }
        };
        file.write_all(lines.as_bytes())
            .with_context(|| self.path.display().to_string())
    }
}

/// Open a session file to read and append, locked; an error if another
/// axon has it open.
fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| path.display().to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!("the session is open in another axon"),
        Err(TryLockError::Error(e)) => Err(e).with_context(|| path.display().to_string()),
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Longest folder name made from a working directory, in bytes.
const MAX_KEY: usize = 100;

/// The folder for sessions started in `cwd`: its path with every character
/// but ASCII letters and digits made a `-`; a long one is cut and ends with
/// a hash of the whole path. Different paths can share a folder, so each
/// session's header says which one it belongs to.
fn dir(root: &Path, cwd: &Path) -> PathBuf {
    let mut key: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if key.len() > MAX_KEY {
        let hash = fnv(cwd.as_os_str().as_encoded_bytes());
        key.truncate(MAX_KEY - 17);
        key.push_str(&format!("-{hash:016x}"));
    }
    root.join(key)
}

/// FNV-1a, which unlike std's hasher stays the same across Rust versions.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The sessions in `dir` started in `cwd`, with when each was last written.
fn saved(dir: &Path, cwd: &Path) -> Result<Vec<(Uuid, SystemTime)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| dir.display().to_string()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(id) = path
            .extension()
            .filter(|e| *e == "jsonl")
            .and(path.file_stem())
            .and_then(|s| s.to_str()?.parse().ok())
        else {
            continue;
        };
        // A session another axon has open may be locked against reading.
        if started_in(&path).ok().flatten().as_deref() == Some(cwd) {
            found.push((id, fs::metadata(&path)?.modified()?));
        }
    }
    Ok(found)
}

/// The working directory in a session file's header.
fn started_in(path: &Path) -> Result<Option<PathBuf>> {
    let file = File::open(path).with_context(|| path.display().to_string())?;
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first)?;
    Ok(match serde_json::from_str::<Entry>(&first) {
        Ok(Entry {
            kind: Kind::Header { cwd, .. },
            ..
        }) => Some(cwd),
        _ => None,
    })
}

/// A session file as read.
#[derive(Debug)]
struct Read {
    saved: Saved,
    /// The id of the last entry kept.
    last: Option<Uuid>,
    /// When the file ends in an incomplete turn: the length without it.
    complete: Option<u64>,
    /// Whether the file is empty or ends in a new line, as appending needs.
    ends_in_newline: bool,
}

/// Read a session file. Entries a newer axon wrote are skipped, unless they
/// are messages: the history needs them all. A last turn without its
/// `TurnEnded` is left out, as it was cut off mid-write.
fn read(text: &str, path: &Path) -> Result<Read> {
    let mut saved = Saved::default();
    let mut last = None;
    // Where the turn being read started, until it ends.
    let mut open: Option<(u64, usize, usize, Option<Uuid>)> = None;
    let mut offset = 0;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (n, line) in lines.iter().enumerate() {
        let start = offset;
        offset += line.len() as u64;
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            if n + 1 < lines.len() {
                bail!("{}: line {} is corrupt", path.display(), n + 1);
            }
            open.get_or_insert((start, saved.past.len(), saved.history.len(), last));
            break;
        };
        let value = migrate(value, MIGRATIONS);
        let entry = match serde_json::from_value::<Entry>(value.clone()) {
            Ok(entry) => entry,
            Err(_) if value["type"] == "message" => {
                let newer = value["version"]
                    .as_str()
                    .and_then(release)
                    .is_some_and(|v| Some(v) > release(VERSION));
                if newer {
                    bail!(
                        "{}: a message written by a newer axon ({})",
                        path.display(),
                        value["version"]
                    );
                }
                bail!(
                    "{}: the message on line {} is unreadable",
                    path.display(),
                    n + 1
                );
            }
            Err(_) => {
                if let Some(id) = value["id"].as_str().and_then(|id| id.parse().ok()) {
                    last = Some(id);
                }
                continue;
            }
        };
        match entry.kind {
            Kind::Header { .. } => {}
            Kind::Prompt { text } => {
                open = Some((start, saved.past.len(), saved.history.len(), last));
                saved.past.push(Past::Prompt(text));
            }
            Kind::Event { event } => {
                if matches!(event, Event::TurnEnded { .. }) {
                    open = None;
                }
                saved.past.push(Past::Event(event));
            }
            Kind::Message { message } => saved.history.push(message),
        }
        last = Some(entry.id);
    }
    let complete = open.map(|(start, past, history, before)| {
        saved.past.truncate(past);
        saved.history.truncate(history);
        last = before;
        start
    });
    Ok(Read {
        saved,
        last,
        complete,
        ends_in_newline: text.is_empty() || text.ends_with('\n'),
    })
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

    fn read_file(path: &Path) -> Result<Read> {
        read(&fs::read_to_string(path).unwrap(), path)
    }

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

    /// A turn that ended, as every turn the session saves does.
    fn ended(prompt: &str) -> Turn {
        let mut turn = Turn::new(prompt.into());
        turn.record(&Event::TurnEnded {
            stop: StopReason::EndTurn,
        });
        turn
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
        let Read {
            saved,
            last,
            complete,
            ..
        } = read_file(&path).unwrap();
        assert_eq!(complete, None);

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
        store.append(ended("one")).unwrap();
        let last = store.last;
        drop(store);

        let (mut store, saved) = Store::open(
            root.path(),
            Path::new("/work/app"),
            "test-model",
            &Resume::Latest,
        )
        .unwrap()
        .unwrap();
        assert_eq!(store.last, last);
        assert_eq!(saved.past[0], Past::Prompt("one".into()));
        store.append(ended("two")).unwrap();

        let text = fs::read_to_string(&store.path).unwrap();
        let types: Vec<_> = masked(&text).iter().map(|v| v["type"].clone()).collect();
        assert_eq!(types, ["header", "prompt", "event", "prompt", "event"]);
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
            store.append(ended(prompt)).unwrap();
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
            entry(json!({"type": "event", "event": {"type": "turn_ended", "stop": "end_turn"}})),
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        let past = read_file(&path).unwrap().saved.past;
        assert_eq!(past[0], Past::Prompt("hi".into()));
        assert_eq!(past.len(), 2);

        let message = entry(json!({"type": "message", "message": {"role": "wizard"}}));
        fs::write(&path, [lines[0].clone(), message.clone()].join("\n")).unwrap();
        let err = read_file(&path).unwrap_err().to_string();
        assert!(err.contains("newer axon") && err.contains("9.0.0"), "{err}");

        let ours = message.replace("9.0.0", VERSION);
        fs::write(&path, [lines[0].clone(), ours].join("\n")).unwrap();
        let err = read_file(&path).unwrap_err().to_string();
        assert!(err.contains("line 2 is unreadable"), "{err}");
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

    fn open_latest(root: &Path, cwd: &str) -> Option<(Store, Saved)> {
        Store::open(root, Path::new(cwd), "m", &Resume::Latest).unwrap()
    }

    #[test]
    fn a_turn_cut_off_mid_write_is_dropped_and_the_next_one_follows_the_last_whole_one() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        write_turns(&mut store);
        let whole = fs::read_to_string(&store.path).unwrap();
        let mut turn = Turn::new("cut".into());
        turn.record(&Event::TextDelta {
            text: "partial".into(),
        });
        turn.record(&Event::TurnEnded {
            stop: StopReason::EndTurn,
        });
        store.append(turn).unwrap();
        let text = fs::read_to_string(&store.path).unwrap();
        let path = store.path.clone();
        drop(store);
        let last_line = text[..text.len() - 1].rfind('\n').unwrap() + 1;
        for cut in [text.len() - 10, last_line, whole.len() + 5] {
            fs::write(&path, &text[..cut]).unwrap();

            let (mut store, saved) = open_latest(root.path(), "/work/app").unwrap();
            assert_eq!(fs::read_to_string(&store.path).unwrap(), whole);
            assert_eq!(
                saved.past.last(),
                Some(&Past::Event(Event::TurnEnded {
                    stop: StopReason::MaxTokens
                }))
            );
            store.append(ended("next")).unwrap();
            let entries: Vec<Value> = fs::read_to_string(&store.path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let [.., before, next] = &entries[..] else {
                panic!()
            };
            assert_eq!(next["parent"], before["id"]);
        }
    }

    #[test]
    fn a_whole_last_turn_without_its_new_line_gets_one_before_the_next() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        store.append(ended("one")).unwrap();
        let text = fs::read_to_string(&store.path).unwrap();
        fs::write(&store.path, text.trim_end()).unwrap();
        drop(store);

        let (mut store, saved) = open_latest(root.path(), "/work/app").unwrap();
        assert_eq!(saved.past[0], Past::Prompt("one".into()));
        store.append(ended("two")).unwrap();
        drop(store);
        let (_, saved) = open_latest(root.path(), "/work/app").unwrap();
        assert_eq!(saved.past[2], Past::Prompt("two".into()));
    }

    #[test]
    fn a_corrupt_line_before_the_end_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        write_turns(&mut store);
        let text = fs::read_to_string(&store.path).unwrap();
        fs::write(&store.path, text.replacen("\"prompt\"", "\"prompt", 1)).unwrap();

        let err = read_file(&store.path).unwrap_err().to_string();
        assert!(err.contains("line 2 is corrupt"), "{err}");
    }

    #[test]
    fn folders_that_share_a_key_keep_their_sessions_apart() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::new(root.path(), Path::new("/a/b-c"), "m");
        store.append(ended("hi")).unwrap();
        drop(store);

        assert_eq!(
            dir(root.path(), Path::new("/a/b-c")),
            dir(root.path(), Path::new("/a/b/c"))
        );
        assert!(open_latest(root.path(), "/a/b/c").is_none());
        assert!(open_latest(root.path(), "/a/b-c").is_some());
    }

    #[test]
    fn a_long_folder_gets_a_short_key_that_still_tells_it_apart() {
        let long = format!("/{}", "é".repeat(300));
        let key = |path: &str| {
            dir(Path::new(""), Path::new(path))
                .to_string_lossy()
                .into_owned()
        };

        assert_eq!(key(&long).len(), MAX_KEY);
        assert_ne!(key(&long), key(&format!("{long}x")));
        assert_eq!(key("/work/app"), "-work-app");
    }

    #[test]
    fn a_new_session_starts_even_when_the_sessions_folder_cannot_be_read() {
        let root = tempfile::NamedTempFile::new().unwrap();

        let opened = Store::open(root.path(), Path::new("/work/app"), "m", &Resume::New);
        assert!(opened.unwrap().is_some());
    }

    #[test]
    fn an_id_prefix_matches_in_any_case_and_cannot_be_empty() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        store.append(ended("hi")).unwrap();
        let open = |prefix: &str| {
            Store::open(
                root.path(),
                Path::new("/work/app"),
                "m",
                &Resume::Id(prefix.into()),
            )
        };

        let upper = store.id.to_string()[..8].to_uppercase();
        let id = store.id;
        drop(store);
        assert_eq!(open(&upper).unwrap().unwrap().0.id, id);
        assert!(open("").is_err());
    }

    #[test]
    fn continue_picks_the_session_written_last_not_the_one_started_last() {
        let root = tempfile::tempdir().unwrap();
        let mut older = new_store(root.path());
        older.append(ended("one")).unwrap();
        let mut newer = new_store(root.path());
        newer.append(ended("two")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        older.append(ended("three")).unwrap();
        let id = older.id;
        drop((older, newer));

        assert_eq!(open_latest(root.path(), "/work/app").unwrap().0.id, id);
    }

    #[test]
    fn a_session_open_in_one_store_cannot_be_opened_in_another() {
        let root = tempfile::tempdir().unwrap();
        let mut store = new_store(root.path());
        store.append(ended("hi")).unwrap();

        let err = Store::open(root.path(), Path::new("/work/app"), "m", &Resume::Latest)
            .unwrap_err()
            .to_string();
        assert!(err.contains("open in another axon"), "{err}");
        drop(store);
        assert!(open_latest(root.path(), "/work/app").is_some());
    }
}
