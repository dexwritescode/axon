//! The tools the model can call, and their schemas.

mod find;
mod grep;
mod list;
mod read;
mod shell;
mod walk;
mod write;

pub(crate) use shell::{Printed, Run};
pub(crate) use write::Write;

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use serde_json::{Value, json};

use crate::jail;

/// The schemas sent with every request.
pub(crate) fn schemas() -> Vec<ChatCompletionTools> {
    vec![
        schema(
            "read_file",
            "Read a text file. Returns up to 2000 lines; use offset to read further. Lines over 2000 characters are cut.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the working directory"},
                    "offset": {"type": "integer", "description": "First line to read, starting at 1"},
                    "limit": {"type": "integer", "description": "Number of lines to read"}
                },
                "required": ["path"]
            }),
        ),
        schema(
            "list_dir",
            "List files and directories, skipping those ignored by .gitignore. Directories end with /.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory relative to the working directory; defaults to it"},
                    "depth": {"type": "integer", "description": "How many levels to descend, 1 to 5; defaults to 2"}
                }
            }),
        ),
        schema(
            "grep",
            "Search file contents with a regex, skipping files ignored by .gitignore. Use it to find where something is instead of reading every file.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regex to search for (Rust regex syntax)"},
                    "path": {"type": "string", "description": "File or directory to search; defaults to the working directory"},
                    "glob": {"type": "string", "description": "Only search files matching this glob, e.g. *.rs; a glob with / matches the path from the working directory"},
                    "ignore_case": {"type": "boolean", "description": "Match case-insensitively; defaults to false"},
                    "output": {"type": "string", "enum": ["lines", "count", "files"], "description": "lines: path:line: text for each match (default); count: matching lines per file; files: paths only"}
                },
                "required": ["pattern"]
            }),
        ),
        schema(
            "write_file",
            "Create a file or replace its whole content; missing directories are created. An existing file must be read first, and read again if it changed since. The user approves every write. To change part of an existing file, use edit_file.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the working directory"},
                    "content": {"type": "string", "description": "The complete new content of the file"}
                },
                "required": ["path", "content"]
            }),
        ),
        schema(
            "edit_file",
            "Replace exact text in an existing file. old_string must match the file exactly, including whitespace, and only once unless replace_all is set; read the file first, and again if it changed since (e.g. after a command). The user approves every edit.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the working directory"},
                    "old_string": {"type": "string", "description": "The exact text to replace"},
                    "new_string": {"type": "string", "description": "The text to put in its place"},
                    "replace_all": {"type": "boolean", "description": "Replace every match instead of requiring a unique one; defaults to false"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        ),
        schema(
            "run_command",
            &format!(
                "Run a command with {} in the working directory. The user approves every command. \
                 stdin is closed; stdout and stderr come back together, capped at {} KB (the first {} KB \
                 and the last {} KB). Background jobs are killed when the command exits.",
                shell::SHELL,
                (shell::HEAD_BYTES + shell::TAIL_BYTES) / 1024,
                shell::HEAD_BYTES / 1024,
                shell::TAIL_BYTES / 1024
            ),
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line"},
                    "timeout": {"type": "integer", "description": format!(
                        "Seconds before the command is killed, 1 to {}; defaults to {}",
                        shell::MAX_TIMEOUT_SECS,
                        shell::DEFAULT_TIMEOUT_SECS
                    )}
                },
                "required": ["command"]
            }),
        ),
        schema(
            "find",
            "Find files by name or path with a glob, skipping those ignored by .gitignore.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob such as *.rs (matches file names) or crates/*/src/*.rs (matches the path from the working directory)"},
                    "path": {"type": "string", "description": "Directory to search; defaults to the working directory"}
                },
                "required": ["pattern"]
            }),
        ),
    ]
}

/// The schemas of the tools in `names`, in the usual order; all of them
/// when `names` is None.
pub(crate) fn enabled(names: Option<&[String]>) -> Vec<ChatCompletionTools> {
    let mut tools = schemas();
    if let Some(names) = names {
        tools.retain(|tool| names.iter().any(|name| name == self::name(tool)));
    }
    tools
}

/// The name a schema gives its tool.
pub(crate) fn name(tool: &ChatCompletionTools) -> &str {
    match tool {
        ChatCompletionTools::Function(tool) => &tool.function.name,
        ChatCompletionTools::Custom(tool) => &tool.custom.name,
    }
}

fn schema(name: &str, description: &str, parameters: Value) -> ChatCompletionTools {
    ChatCompletionTools::Function(ChatCompletionTool {
        function: FunctionObject {
            name: name.into(),
            description: Some(description.into()),
            parameters: Some(parameters),
            strict: None,
        },
    })
}

/// What a tool call produced.
#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    /// The tool ran; this is its output.
    Output(String),
    /// `read_file` ran: its output, and the file as the model now knows it.
    Read { output: String, seen: Seen },
    /// The tool wants to change a file, once the user approves.
    Write(Write),
    /// The tool wants to run a command, once the user approves.
    Run(Run),
}

/// Run a tool call inside `cwd`: what it produced, or an error message for the model.
pub(crate) fn run(cwd: &Path, name: &str, args: &str) -> Result<Action, String> {
    let args: Value = if args.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(args).map_err(|e| format!("arguments were not valid JSON: {e}"))?
    };
    match name {
        "read_file" => read::run(cwd, &args).map(|(output, seen)| Action::Read { output, seen }),
        "list_dir" => list::run(cwd, &args).map(Action::Output),
        "grep" => grep::run(cwd, &args).map(Action::Output),
        "find" => find::run(cwd, &args).map(Action::Output),
        "write_file" => write::write_file(cwd, &args).map(Action::Write),
        "edit_file" => write::edit_file(cwd, &args).map(Action::Write),
        "run_command" => shell::run_command(&args).map(Action::Run),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn optional_usize(args: &Value, name: &str) -> Result<Option<usize>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| format!("{name} must be a non-negative integer")),
    }
}

fn optional_bool(args: &Value, name: &str) -> Result<Option<bool>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{name} must be true or false")),
    }
}

fn optional_str<'a>(args: &'a Value, name: &str) -> Result<Option<&'a str>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("{name} must be a string")),
    }
}

fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    optional_str(args, name)?.ok_or_else(|| format!("{name} is required"))
}

/// The directory at `path`, inside the working directory.
fn resolve_dir(cwd: &Path, path: &str) -> Result<PathBuf, String> {
    let dir = jail::resolve(cwd, path)?;
    if !dir.is_dir() {
        return Err(format!("{path}: not a directory"));
    }
    Ok(dir)
}

/// The first `max` chars of `text`, if it is longer.
fn cut_chars(text: &str, max: usize) -> Option<&str> {
    text.char_indices().nth(max).map(|(end, _)| &text[..end])
}

/// A file's content as the model last read or wrote it, by hash, to tell
/// whether the file changed since.
#[derive(Debug, PartialEq)]
pub(crate) struct Seen {
    pub(crate) target: PathBuf,
    pub(crate) hash: u64,
}

impl Seen {
    fn new(target: &Path, text: &str) -> Self {
        Self {
            target: target.to_path_buf(),
            hash: hash(text),
        }
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The text of the file at `target`; `path` is how errors name it.
fn read_text(target: &Path, path: &str) -> Result<String, String> {
    let bytes = fs::read(target).map_err(|e| format!("{path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path}: not a UTF-8 text file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_json_is_reported_to_the_model() {
        let error = run(Path::new("."), "read_file", "{not json").unwrap_err();

        assert!(
            error.starts_with("arguments were not valid JSON"),
            "{error}"
        );
    }

    #[test]
    fn empty_arguments_are_an_empty_object() {
        for args in ["", "  "] {
            assert_eq!(
                run(Path::new("."), "read_file", args),
                Err("path is required".into())
            );
        }
    }

    #[test]
    fn enabled_keeps_the_named_tools_in_order() {
        let names = |tools: Vec<ChatCompletionTools>| -> Vec<String> {
            tools.iter().map(|tool| name(tool).to_string()).collect()
        };

        assert_eq!(names(enabled(None)), names(schemas()));
        assert_eq!(
            names(enabled(Some(&["grep".into(), "read_file".into()]))),
            ["read_file", "grep"]
        );
        assert!(enabled(Some(&[])).is_empty());
    }

    #[test]
    fn unknown_tool_is_an_error() {
        assert_eq!(
            run(Path::new("."), "rm", "{}"),
            Err("unknown tool: rm".into())
        );
    }

    #[test]
    fn every_schema_names_a_tool_that_runs() {
        for tool in schemas() {
            let ChatCompletionTools::Function(tool) = tool else {
                panic!("not a function tool");
            };
            let error = run(Path::new("."), &tool.function.name, "{}").err();
            assert_ne!(error, Some(format!("unknown tool: {}", tool.function.name)));
        }
    }
}
