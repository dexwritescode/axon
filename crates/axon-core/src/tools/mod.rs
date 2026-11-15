//! The tools the model can call, and their schemas.

mod list;
mod read;

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

/// Run a tool call inside `cwd`: its output, or an error message for the model.
pub(crate) fn run(cwd: &Path, name: &str, args: &str) -> Result<String, String> {
    let args: Value = if args.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(args).map_err(|e| format!("arguments were not valid JSON: {e}"))?
    };
    match name {
        "read_file" => read::run(cwd, &args),
        "list_dir" => list::run(cwd, &args),
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
