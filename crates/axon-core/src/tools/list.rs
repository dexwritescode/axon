//! `list_dir`: the tree under a directory, respecting `.gitignore`.

use std::path::Path;

use serde_json::Value;

use super::walk::{listing, relative, walk};
use super::{optional_str, optional_usize, resolve_dir};

const DEFAULT_DEPTH: usize = 2;
const MAX_DEPTH: usize = 5;
const MAX_ENTRIES: usize = 500;

pub(super) fn run(cwd: &Path, args: &Value) -> Result<String, String> {
    let path = optional_str(args, "path")?.unwrap_or(".");
    let depth = optional_usize(args, "depth")?
        .unwrap_or(DEFAULT_DEPTH)
        .clamp(1, MAX_DEPTH);

    let dir = resolve_dir(cwd, path)?;

    let mut entries = Vec::new();
    for entry in walk(&dir, Some(depth)) {
        let entry = entry.map_err(|e| format!("{path}: {e}"))?;
        if entry.path() == dir {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let suffix = if is_dir { "/" } else { "" };
        entries.push(format!("{}{suffix}", relative(cwd, entry.path())));
    }
    Ok(listing(
        &entries,
        MAX_ENTRIES,
        "(empty)",
        "list a subdirectory or lower the depth",
    ))
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
            Ok("src/deep/\nsrc/main.rs\n".into())
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
        assert!(out.ends_with("[showing 500 of 510; list a subdirectory or lower the depth]\n"));
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
