use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, anyhow};
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use serde_json::{Value, json};

pub struct ToolExecutor {
    working_dir: PathBuf,
}

impl ToolExecutor {
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
        }
    }

    pub fn execute(&self, name: &str, args: &Value) -> Result<String> {
        match name {
            "read_file" => self.read_file(args),
            "edit_file" => self.edit_file(args),
            "shell" => self.shell(args),
            other => Err(anyhow!("unknown tool: {other}")),
        }
    }

    fn read_file(&self, args: &Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing path"))?;
        Ok(std::fs::read_to_string(self.working_dir.join(path))?)
    }

    fn edit_file(&self, args: &Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("missing path"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow!("missing content"))?;
        let full_path = self.working_dir.join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, content)?;
        Ok(format!("wrote {path}"))
    }

    fn shell(&self, args: &Value) -> Result<String> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow!("missing command"))?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.working_dir)
            .output()?;
        let mut result = String::from_utf8_lossy(&output.stdout).into_owned();
        result.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(result)
    }
}

/// OpenAI tool schemas passed in every chat completion request.
pub fn tool_schemas() -> Vec<ChatCompletionTools> {
    vec![
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "read_file".into(),
                description: Some("Read a file from disk and return its full contents.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the working directory"
                        }
                    },
                    "required": ["path"]
                })),
                strict: None,
            },
        }),
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "edit_file".into(),
                description: Some(
                    "Write content to a file, creating or replacing it entirely.".into(),
                ),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the working directory"
                        },
                        "content": {
                            "type": "string",
                            "description": "Full content to write"
                        }
                    },
                    "required": ["path", "content"]
                })),
                strict: None,
            },
        }),
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "shell".into(),
                description: Some(
                    "Run a shell command in the working directory. Returns stdout and stderr combined.".into(),
                ),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute"
                        }
                    },
                    "required": ["command"]
                })),
                strict: None,
            },
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> (TempDir, ToolExecutor) {
        let dir = TempDir::new().unwrap();
        let executor = ToolExecutor::new(dir.path());
        (dir, executor)
    }

    #[test]
    fn read_file_returns_contents() {
        let (dir, executor) = tmp();
        fs::write(dir.path().join("hello.txt"), "hello axon").unwrap();
        let result = executor
            .execute("read_file", &json!({"path": "hello.txt"}))
            .unwrap();
        assert_eq!(result, "hello axon");
    }

    #[test]
    fn edit_file_writes_and_confirms() {
        let (dir, executor) = tmp();
        let result = executor
            .execute(
                "edit_file",
                &json!({"path": "out.txt", "content": "written"}),
            )
            .unwrap();
        assert_eq!(result, "wrote out.txt");
        assert_eq!(
            fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "written"
        );
    }

    #[test]
    fn edit_file_creates_parent_dirs() {
        let (dir, executor) = tmp();
        executor
            .execute(
                "edit_file",
                &json!({"path": "a/b/c.txt", "content": "deep"}),
            )
            .unwrap();
        assert!(dir.path().join("a/b/c.txt").exists());
    }

    #[test]
    fn shell_returns_stdout() {
        let (_dir, executor) = tmp();
        let out = executor
            .execute("shell", &json!({"command": "echo axon-test"}))
            .unwrap();
        assert!(out.contains("axon-test"));
    }

    #[test]
    fn shell_captures_stderr() {
        let (_dir, executor) = tmp();
        let out = executor
            .execute("shell", &json!({"command": "echo err >&2"}))
            .unwrap();
        assert!(out.contains("err"));
    }

    #[test]
    fn unknown_tool_returns_error() {
        let (_dir, executor) = tmp();
        assert!(executor.execute("nonexistent", &json!({})).is_err());
    }

    #[test]
    fn tool_schemas_has_three_tools_in_order() {
        let schemas = tool_schemas();
        assert_eq!(schemas.len(), 3);
        let names: Vec<&str> = schemas
            .iter()
            .map(|t| match t {
                ChatCompletionTools::Function(f) => f.function.name.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(names, ["read_file", "edit_file", "shell"]);
    }
}
