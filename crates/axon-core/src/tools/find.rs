//! `find`: files whose name or path matches a glob, respecting `.gitignore`.

use std::path::Path;

use serde_json::Value;

use super::optional_str;
use super::walk::{Glob, listing, relative, walk};
use crate::jail;

const MAX_FILES: usize = 500;

pub(super) fn run(cwd: &Path, args: &Value) -> Result<String, String> {
    let pattern = optional_str(args, "pattern")?.ok_or("pattern is required")?;
    let path = optional_str(args, "path")?.unwrap_or(".");
    let glob = Glob::new(pattern)?;

    let dir = jail::resolve(cwd, path)?;
    if !dir.is_dir() {
        return Err(format!("{path}: not a directory"));
    }

    let files: Vec<String> = walk(&dir, None)
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(|entry| relative(cwd, entry.path()))
        .filter(|rel| glob.matches(rel))
        .collect();
    Ok(listing(
        &files,
        MAX_FILES,
        "(no matches)",
        "narrow the pattern or path",
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
        fs::create_dir_all(root.join("crates/core/src/tools")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("crates/core/Cargo.toml"), "").unwrap();
        fs::write(root.join("crates/core/src/lib.rs"), "").unwrap();
        fs::write(root.join("crates/core/src/tools/mod.rs"), "").unwrap();
        fs::write(root.join("target/gen.rs"), "").unwrap();
        dir
    }

    fn find(dir: &TempDir, args: Value) -> Result<String, String> {
        run(&dir.path().canonicalize().unwrap(), &args)
    }

    #[test]
    fn names_match_at_any_depth_skipping_ignored() {
        let dir = tree();

        assert_eq!(
            find(&dir, json!({"pattern": "*.rs"})),
            Ok("crates/core/src/lib.rs\ncrates/core/src/tools/mod.rs\n".into())
        );
        assert_eq!(
            find(&dir, json!({"pattern": "Cargo.toml", "path": "crates"})),
            Ok("crates/core/Cargo.toml\n".into())
        );
    }

    #[test]
    fn patterns_with_a_slash_match_the_path() {
        let dir = tree();

        assert_eq!(
            find(&dir, json!({"pattern": "crates/*/src/*.rs"})),
            Ok("crates/core/src/lib.rs\n".into())
        );
    }

    #[test]
    fn many_files_are_capped() {
        let dir = TempDir::new().unwrap();
        for n in 0..MAX_FILES + 3 {
            fs::write(dir.path().join(format!("f{n:04}.txt")), "").unwrap();
        }

        let out = find(&dir, json!({"pattern": "*.txt"})).unwrap();
        assert!(out.ends_with("[showing 500 of 503; narrow the pattern or path]\n"));
    }

    #[test]
    fn no_matches_and_bad_arguments() {
        let dir = tree();

        assert_eq!(
            find(&dir, json!({"pattern": "*.py"})),
            Ok("(no matches)\n".into())
        );
        assert!(
            find(&dir, json!({"pattern": "*", "path": ".."}))
                .unwrap_err()
                .contains("outside")
        );
        assert!(
            find(&dir, json!({"pattern": "*", "path": "Cargo.toml"}))
                .unwrap_err()
                .contains("not a directory")
        );
        assert_eq!(find(&dir, json!({})), Err("pattern is required".into()));
    }
}
