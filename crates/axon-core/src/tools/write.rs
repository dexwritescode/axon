//! `write_file` and `edit_file`: both work out the new text and return a
//! `Write` to approve; nothing touches the disk until it is applied.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::walk::relative;
use super::{optional_bool, optional_str};
use crate::jail;

/// A file change waiting for approval.
#[derive(Debug, PartialEq)]
pub(crate) struct Write {
    /// Relative to the working directory.
    pub(crate) path: String,
    target: PathBuf,
    pub(crate) old_text: Option<String>,
    pub(crate) new_text: String,
}

impl Write {
    /// Write the file, creating missing parent directories.
    pub(crate) fn apply(&self) -> Result<String, String> {
        if let Some(dir) = self.target.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", self.path))?;
        }
        fs::write(&self.target, &self.new_text).map_err(|e| format!("{}: {e}", self.path))?;
        Ok(format!("wrote {}", self.path))
    }
}

pub(super) fn write_file(cwd: &Path, args: &Value) -> Result<Write, String> {
    let path = required_str(args, "path")?;
    let content = required_str(args, "content")?;

    let target = jail::resolve_new(cwd, path)?;
    if target.is_dir() {
        return Err(format!("{path}: is a directory"));
    }
    let old_text = if target.exists() {
        Some(read_text(&target, path)?)
    } else {
        None
    };
    Ok(Write {
        path: relative(cwd, &target),
        target,
        old_text,
        new_text: content.into(),
    })
}

pub(super) fn edit_file(cwd: &Path, args: &Value) -> Result<Write, String> {
    let path = required_str(args, "path")?;
    let old_string = required_str(args, "old_string")?;
    let new_string = required_str(args, "new_string")?;
    let replace_all = optional_bool(args, "replace_all")?.unwrap_or(false);
    if old_string.is_empty() {
        return Err("old_string must not be empty".into());
    }
    if old_string == new_string {
        return Err("new_string is the same as old_string".into());
    }

    let target = jail::resolve(cwd, path)?;
    let text = read_text(&target, path)?;
    let new_text = match text.matches(old_string).count() {
        0 => return Err(format!("old_string not found in {path}")),
        1 => text.replacen(old_string, new_string, 1),
        _ if replace_all => text.replace(old_string, new_string),
        n => {
            return Err(format!(
                "old_string matches {n} times in {path}; add surrounding lines to make it unique or set replace_all"
            ));
        }
    };
    Ok(Write {
        path: relative(cwd, &target),
        target,
        old_text: Some(text),
        new_text,
    })
}

fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    optional_str(args, name)?.ok_or_else(|| format!("{name} is required"))
}

fn read_text(target: &Path, path: &str) -> Result<String, String> {
    let bytes = fs::read(target).map_err(|e| format!("{path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path}: not a UTF-8 text file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn dir_with_notes() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        fs::write(cwd.join("notes.txt"), "milk\neggs\nmilk\n").unwrap();
        (dir, cwd)
    }

    #[test]
    fn write_file_to_a_new_path_creates_it_and_its_directories() {
        let (_dir, cwd) = dir_with_notes();

        let write = write_file(&cwd, &json!({"path": "a/b/new.txt", "content": "hi\n"})).unwrap();
        assert_eq!(
            (
                write.path.as_str(),
                write.old_text.as_deref(),
                write.new_text.as_str()
            ),
            ("a/b/new.txt", None, "hi\n")
        );
        assert!(!cwd.join("a").exists(), "nothing is written before apply");

        assert_eq!(write.apply(), Ok("wrote a/b/new.txt".into()));
        assert_eq!(fs::read_to_string(cwd.join("a/b/new.txt")).unwrap(), "hi\n");
    }

    #[test]
    fn write_file_over_an_existing_file_carries_its_old_text() {
        let (_dir, cwd) = dir_with_notes();

        let write = write_file(&cwd, &json!({"path": "./notes.txt", "content": "tea\n"})).unwrap();

        assert_eq!(write.path, "notes.txt");
        assert_eq!(write.old_text.as_deref(), Some("milk\neggs\nmilk\n"));
    }

    #[test]
    fn write_file_rejects_directories_escapes_and_missing_args() {
        let (_dir, cwd) = dir_with_notes();
        fs::create_dir(cwd.join("sub")).unwrap();
        let write = |args| write_file(&cwd, &args).unwrap_err();

        assert!(write(json!({"path": "sub", "content": ""})).contains("is a directory"));
        assert!(write(json!({"path": "../x", "content": ""})).contains("outside"));
        assert_eq!(write(json!({"path": "x"})), "content is required");
    }

    #[test]
    fn edit_file_replaces_a_unique_match() {
        let (_dir, cwd) = dir_with_notes();

        let write = edit_file(
            &cwd,
            &json!({"path": "notes.txt", "old_string": "eggs", "new_string": "bread"}),
        )
        .unwrap();

        assert_eq!(write.old_text.as_deref(), Some("milk\neggs\nmilk\n"));
        assert_eq!(write.new_text, "milk\nbread\nmilk\n");
    }

    #[test]
    fn edit_file_needs_replace_all_for_several_matches() {
        let (_dir, cwd) = dir_with_notes();
        let args = json!({"path": "notes.txt", "old_string": "milk", "new_string": "tea"});

        assert!(
            edit_file(&cwd, &args)
                .unwrap_err()
                .starts_with("old_string matches 2 times in notes.txt")
        );

        let mut args = args;
        args["replace_all"] = json!(true);
        assert_eq!(edit_file(&cwd, &args).unwrap().new_text, "tea\neggs\ntea\n");
    }

    #[test]
    fn edit_file_errors_before_asking() {
        let (_dir, cwd) = dir_with_notes();
        let edit = |args| edit_file(&cwd, &args).unwrap_err();

        assert_eq!(
            edit(json!({"path": "notes.txt", "old_string": "tea", "new_string": "x"})),
            "old_string not found in notes.txt"
        );
        assert_eq!(
            edit(json!({"path": "notes.txt", "old_string": "", "new_string": "x"})),
            "old_string must not be empty"
        );
        assert_eq!(
            edit(json!({"path": "notes.txt", "old_string": "milk", "new_string": "milk"})),
            "new_string is the same as old_string"
        );
        assert!(
            edit(json!({"path": "new.txt", "old_string": "a", "new_string": "b"}))
                .starts_with("new.txt: ")
        );
    }
}
