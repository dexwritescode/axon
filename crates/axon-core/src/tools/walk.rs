//! What `list_dir`, `grep` and `find` share: the `.gitignore`-aware walk,
//! glob patterns, output paths and capped listings.

use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use ignore::{Walk, WalkBuilder};

/// Everything under `dir` in name order, skipping `.git` and ignored files.
pub(super) fn walk(dir: &Path, max_depth: Option<usize>) -> Walk {
    WalkBuilder::new(dir)
        .max_depth(max_depth)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build()
}

/// `path` as the model should write it: relative to the working directory.
pub(super) fn relative(cwd: &Path, path: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// A glob matched against file names, or against the whole relative path
/// when the pattern contains a `/`.
pub(super) struct Glob {
    matcher: GlobMatcher,
    on_path: bool,
}

impl Glob {
    pub(super) fn new(pattern: &str) -> Result<Self, String> {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("invalid glob: {e}"))?;
        Ok(Self {
            matcher: glob.compile_matcher(),
            on_path: pattern.contains('/'),
        })
    }

    pub(super) fn matches(&self, relative: &str) -> bool {
        if self.on_path {
            self.matcher.is_match(relative)
        } else {
            let name = relative.rsplit('/').next().unwrap_or(relative);
            self.matcher.is_match(name)
        }
    }
}

/// One entry per line, at most `max`, with a note saying how to see the rest.
pub(super) fn listing(entries: &[String], max: usize, empty: &str, narrow: &str) -> String {
    if entries.is_empty() {
        return format!("{empty}\n");
    }
    let mut out: String = entries
        .iter()
        .take(max)
        .map(|entry| format!("{entry}\n"))
        .collect();
    if entries.len() > max {
        out.push_str(&format!("[showing {max} of {}; {narrow}]\n", entries.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_without_a_slash_match_names_anywhere() {
        let glob = Glob::new("*.rs").unwrap();

        assert!(glob.matches("main.rs"));
        assert!(glob.matches("crates/core/src/lib.rs"));
        assert!(!glob.matches("Cargo.toml"));
    }

    #[test]
    fn globs_with_a_slash_match_the_path() {
        let glob = Glob::new("crates/*/src/*.rs").unwrap();

        assert!(glob.matches("crates/core/src/lib.rs"));
        assert!(!glob.matches("crates/core/src/tools/mod.rs"));
        assert!(!glob.matches("lib.rs"));
        assert!(
            Glob::new("crates/**/*.rs")
                .unwrap()
                .matches("crates/core/src/tools/mod.rs")
        );
    }

    #[test]
    fn invalid_globs_are_errors() {
        assert!(Glob::new("[").err().unwrap().starts_with("invalid glob"));
    }

    #[test]
    fn listings_are_capped_with_a_note() {
        let entries: Vec<String> = ["a", "b", "c"].map(String::from).into();

        assert_eq!(listing(&entries, 5, "(none)", "narrow it"), "a\nb\nc\n");
        assert_eq!(
            listing(&entries, 2, "(none)", "narrow it"),
            "a\nb\n[showing 2 of 3; narrow it]\n"
        );
        assert_eq!(listing(&[], 2, "(none)", "narrow it"), "(none)\n");
    }

    #[test]
    fn paths_are_relative_to_the_working_directory() {
        let cwd = Path::new("/work");

        assert_eq!(relative(cwd, Path::new("/work/src/main.rs")), "src/main.rs");
        assert_eq!(relative(cwd, Path::new("/work")), ".");
    }
}
