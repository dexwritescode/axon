//! `grep`: search file contents with a regex, respecting `.gitignore`.

use std::fs;
use std::path::Path;

use regex::RegexBuilder;
use serde_json::Value;

use super::walk::{Glob, listing, relative, walk};
use super::{optional_bool, optional_str};
use crate::jail;

const MAX_RESULTS: usize = 200;
const MAX_LINE_CHARS: usize = 300;

enum Output {
    Lines,
    Count,
    Files,
}

pub(super) fn run(cwd: &Path, args: &Value) -> Result<String, String> {
    let pattern = optional_str(args, "pattern")?.ok_or("pattern is required")?;
    let path = optional_str(args, "path")?.unwrap_or(".");
    let glob = optional_str(args, "glob")?.map(Glob::new).transpose()?;
    let ignore_case = optional_bool(args, "ignore_case")?.unwrap_or(false);
    let output = match optional_str(args, "output")?.unwrap_or("lines") {
        "lines" => Output::Lines,
        "count" => Output::Count,
        "files" => Output::Files,
        other => return Err(format!("output must be lines, count or files, not {other}")),
    };
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("invalid pattern: {e}"))?;

    let root = jail::resolve(cwd, path)?;
    let mut results = Vec::new();
    for entry in walk(&root, None).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = relative(cwd, entry.path());
        if glob.as_ref().is_some_and(|glob| !glob.matches(&rel)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut matches = text
            .lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line));
        match output {
            Output::Lines => {
                for (n, line) in matches {
                    results.push(format!("{rel}:{}: {}", n + 1, clip(line)));
                }
            }
            Output::Count => match matches.count() {
                0 => {}
                count => results.push(format!("{rel}: {count}")),
            },
            Output::Files => {
                if matches.next().is_some() {
                    results.push(rel);
                }
            }
        }
    }
    Ok(listing(
        &results,
        MAX_RESULTS,
        "(no matches)",
        "narrow the pattern, path or glob",
    ))
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("README.md"), "struct in prose\n").unwrap();
        fs::write(root.join("src/a.rs"), "struct A;\nfn a() {}\nstruct B;\n").unwrap();
        fs::write(root.join("src/b.rs"), "fn b() {}\nSTRUCT shouting\n").unwrap();
        fs::write(root.join("src/bin.dat"), [0xff, 0xfe, b's']).unwrap();
        fs::write(root.join("target/gen.rs"), "struct Gen;\n").unwrap();
        fs::write(root.join(".git/config"), "struct\n").unwrap();
        dir
    }

    fn grep(dir: &TempDir, args: Value) -> Result<String, String> {
        run(&dir.path().canonicalize().unwrap(), &args)
    }

    #[test]
    fn lines_show_path_and_line_number_skipping_ignored_and_binary() {
        let dir = tree();

        assert_eq!(
            grep(&dir, json!({"pattern": "struct"})),
            Ok(
                "README.md:1: struct in prose\nsrc/a.rs:1: struct A;\nsrc/a.rs:3: struct B;\n"
                    .into()
            )
        );
    }

    #[test]
    fn count_and_files_summarise_per_file() {
        let dir = tree();

        assert_eq!(
            grep(&dir, json!({"pattern": "struct", "output": "count"})),
            Ok("README.md: 1\nsrc/a.rs: 2\n".into())
        );
        assert_eq!(
            grep(&dir, json!({"pattern": "struct", "output": "files"})),
            Ok("README.md\nsrc/a.rs\n".into())
        );
    }

    #[test]
    fn path_glob_and_case_narrow_the_search() {
        let dir = tree();

        assert_eq!(
            grep(
                &dir,
                json!({"pattern": "struct", "glob": "*.rs", "ignore_case": true, "output": "files"})
            ),
            Ok("src/a.rs\nsrc/b.rs\n".into())
        );
        assert_eq!(
            grep(&dir, json!({"pattern": "fn", "path": "src/b.rs"})),
            Ok("src/b.rs:1: fn b() {}\n".into())
        );
    }

    #[test]
    fn long_lines_and_many_results_are_capped() {
        let dir = TempDir::new().unwrap();
        let long = "x".repeat(MAX_LINE_CHARS + 50);
        let text = format!("{long}\n").repeat(MAX_RESULTS + 5);
        fs::write(dir.path().join("big.txt"), text).unwrap();

        let out = grep(&dir, json!({"pattern": "x"})).unwrap();
        let mut lines = out.lines();
        assert_eq!(
            lines.next(),
            Some(format!("big.txt:1: {}…", "x".repeat(MAX_LINE_CHARS)).as_str())
        );
        assert_eq!(
            lines.last(),
            Some("[showing 200 of 205; narrow the pattern, path or glob]")
        );
    }

    #[test]
    fn no_matches_and_bad_arguments() {
        let dir = tree();

        assert_eq!(
            grep(&dir, json!({"pattern": "nothing here"})),
            Ok("(no matches)\n".into())
        );
        assert!(
            grep(&dir, json!({"pattern": "("}))
                .unwrap_err()
                .starts_with("invalid pattern")
        );
        assert!(
            grep(&dir, json!({"pattern": "x", "output": "all"}))
                .unwrap_err()
                .starts_with("output must be")
        );
        assert!(
            grep(&dir, json!({"pattern": "x", "path": ".."}))
                .unwrap_err()
                .contains("outside")
        );
        assert_eq!(grep(&dir, json!({})), Err("pattern is required".into()));
    }
}
