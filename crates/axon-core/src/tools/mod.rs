//! The tools the model can call, and their schemas.

mod find;
mod grep;
mod list;
mod read;
mod walk;
mod write;

pub(crate) use write::Write;

use std::path::Path;

use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use serde_json::{Value, json};

/// The schemas sent with every request.
pub(crate) fn schemas() -> Vec<ChatCompletionTools> {
    vec![
        schema(
            "read_file",
            "Read a text file. Returns up to 2000 lines; use offset to read further.",
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
            "Create a file or replace its whole content; missing directories are created. The user approves every write. To change part of an existing file, use edit_file.",
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
            "Replace exact text in an existing file. old_string must match the file exactly, including whitespace, and only once unless replace_all is set; read the file first. The user approves every edit.",
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
    /// The tool wants to change a file, once the user approves.
    Write(Write),
}

/// Run a tool call inside `cwd`: what it produced, or an error message for the model.
pub(crate) fn run(cwd: &Path, name: &str, args: &str) -> Result<Action, String> {
    let args: Value = if args.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(args).map_err(|e| format!("arguments were not valid JSON: {e}"))?
    };
    match name {
        "read_file" => read::run(cwd, &args).map(Action::Output),
        "list_dir" => list::run(cwd, &args).map(Action::Output),
        "grep" => grep::run(cwd, &args).map(Action::Output),
        "find" => find::run(cwd, &args).map(Action::Output),
        "write_file" => write::write_file(cwd, &args).map(Action::Write),
        "edit_file" => write::edit_file(cwd, &args).map(Action::Write),
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
