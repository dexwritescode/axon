//! `list_dir`: the tree under a directory, respecting `.gitignore`.

use std::path::Path;

use ignore::WalkBuilder;
use serde_json::Value;

use super::{optional_str, optional_usize};
use crate::jail;

const DEFAULT_DEPTH: usize = 2;
const MAX_DEPTH: usize = 5;
const MAX_ENTRIES: usize = 500;

pub(super) fn run(cwd: &Path, args: &Value) -> Result<String, String> {
    let path = optional_str(args, "path")?.unwrap_or(".");
    let depth = optional_usize(args, "depth")?
        .unwrap_or(DEFAULT_DEPTH)
        .clamp(1, MAX_DEPTH);

    let dir = jail::resolve(cwd, path)?;
    if !dir.is_dir() {
        return Err(format!("{path}: not a directory"));
    }

    let walk = WalkBuilder::new(&dir)
        .max_depth(Some(depth))
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let mut entries = Vec::new();
    for entry in walk {
        let entry = entry.map_err(|e| e.to_string())?;
        let Ok(relative) = entry.path().strip_prefix(&dir) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let suffix = if is_dir { "/" } else { "" };
        entries.push(format!("{}{suffix}", relative.display()));
    }

    let total = entries.len();
    if total == 0 {
        return Ok("(empty)\n".into());
    }
    let mut out: String = entries
        .iter()
        .take(MAX_ENTRIES)
        .map(|entry| format!("{entry}\n"))
        .collect();
    if total > MAX_ENTRIES {
        out.push_str(&format!(
            "[showing {MAX_ENTRIES} of {total} entries; list a subdirectory or lower the depth]\n"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/deep/deeper/x.rs"), "").unwrap();
        dir
    }

    fn list(dir: &TempDir, args: Value) -> Result<String, String> {
        run(&dir.path().canonicalize().unwrap(), &args)
    }

    #[test]
    fn lists_two_levels_skipping_ignored_and_git() {
        let dir = tree();

        assert_eq!(
            list(&dir, json!({})),
            Ok(".gitignore\nCargo.toml\nsrc/\nsrc/deep/\nsrc/main.rs\n".into())
        );
    }

    #[test]
    fn path_and_depth_narrow_the_listing() {
        let dir = tree();

        assert_eq!(
            list(&dir, json!({"path": "src", "depth": 1})),
            Ok("deep/\nmain.rs\n".into())
        );
    }

    #[test]
    fn large_listings_are_capped() {
        let dir = TempDir::new().unwrap();
        for n in 0..MAX_ENTRIES + 10 {
            fs::write(dir.path().join(format!("f{n:04}")), "").unwrap();
        }

        let out = list(&dir, json!({})).unwrap();
        assert_eq!(out.lines().count(), MAX_ENTRIES + 1);
        assert!(
            out.ends_with("[showing 500 of 510 entries; list a subdirectory or lower the depth]\n")
        );
    }

    #[test]
    fn files_and_escapes_are_errors() {
        let dir = tree();

        assert!(
            list(&dir, json!({"path": "Cargo.toml"}))
                .unwrap_err()
                .contains("not a directory")
        );
        assert!(
            list(&dir, json!({"path": ".."}))
                .unwrap_err()
                .contains("outside")
        );
    }
}
