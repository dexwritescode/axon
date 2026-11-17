//! `read_file`: a range of lines from a text file.

use std::path::Path;

use serde_json::Value;

use super::{optional_usize, read_text, required_str};
use crate::jail;

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 64 * 1024;

pub(super) fn run(cwd: &Path, args: &Value) -> Result<String, String> {
    let path = required_str(args, "path")?;
    let offset = optional_usize(args, "offset")?.unwrap_or(1).max(1);
    let limit = optional_usize(args, "limit")?
        .unwrap_or(MAX_LINES)
        .clamp(1, MAX_LINES);

    let file = jail::resolve(cwd, path)?;
    let text = read_text(&file, path)?;

    let lines: Vec<&str> = text.lines().collect();
    let first = offset - 1;
    if first >= lines.len() && !lines.is_empty() {
        return Err(format!(
            "{path}: offset {offset} is past the last line ({})",
            lines.len()
        ));
    }

    if let Some(line) = lines.get(first).filter(|line| line.len() + 1 > MAX_BYTES) {
        let mut end = MAX_BYTES;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        return Ok(format!(
            "{}\n[line {offset} cut at {end} bytes; use offset {} to read more]\n",
            &line[..end],
            offset + 1
        ));
    }

    let mut out = String::new();
    let mut shown = 0;
    for line in lines.iter().skip(first).take(limit) {
        if out.len() + line.len() + 1 > MAX_BYTES {
            break;
        }
        out.push_str(line);
        out.push('\n');
        shown += 1;
    }
    let last = first + shown;
    if last < lines.len() {
        out.push_str(&format!(
            "[showing lines {offset}-{last} of {}; use offset {} to read more]\n",
            lines.len(),
            last + 1
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

    fn dir_with(name: &str, contents: &[u8]) -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(name), contents).unwrap();
        dir
    }

    fn read(dir: &TempDir, args: Value) -> Result<String, String> {
        run(&dir.path().canonicalize().unwrap(), &args)
    }

    #[test]
    fn reads_the_whole_file() {
        let dir = dir_with("a.txt", b"one\ntwo\n");

        assert_eq!(
            read(&dir, json!({"path": "a.txt"})),
            Ok("one\ntwo\n".into())
        );
    }

    #[test]
    fn offset_and_limit_select_lines_and_note_the_rest() {
        let dir = dir_with("a.txt", b"1\n2\n3\n4\n");

        assert_eq!(
            read(&dir, json!({"path": "a.txt", "offset": 2, "limit": 2})),
            Ok("2\n3\n[showing lines 2-3 of 4; use offset 4 to read more]\n".into())
        );
    }

    #[test]
    fn long_files_are_capped() {
        let contents: String = (1..=3000).map(|n| format!("{n}\n")).collect();
        let dir = dir_with("a.txt", contents.as_bytes());

        let out = read(&dir, json!({"path": "a.txt"})).unwrap();
        assert_eq!(out.lines().count(), MAX_LINES + 1);
        assert!(out.ends_with("[showing lines 1-2000 of 3000; use offset 2001 to read more]\n"));
    }

    #[test]
    fn output_is_capped_in_bytes() {
        let line = "x".repeat(1000);
        let contents: String = (0..100).map(|_| format!("{line}\n")).collect();
        let dir = dir_with("a.txt", contents.as_bytes());

        let out = read(&dir, json!({"path": "a.txt"})).unwrap();
        assert!(out.len() <= MAX_BYTES + 100);
        assert!(
            out.contains("use offset 66 to read more"),
            "{}",
            &out[out.len() - 80..]
        );
    }

    #[test]
    fn errors_name_the_problem() {
        let dir = dir_with("bin", &[0xff, 0xfe]);

        assert_eq!(read(&dir, json!({})), Err("path is required".into()));
        assert!(
            read(&dir, json!({"path": "bin"}))
                .unwrap_err()
                .contains("not a UTF-8")
        );
        assert!(read(&dir, json!({"path": "../x"})).is_err());
        assert!(
            read(&dir, json!({"path": "bin", "offset": "2"}))
                .unwrap_err()
                .contains("offset")
        );
    }

    #[test]
    fn an_empty_file_reads_as_empty() {
        let dir = dir_with("a.txt", b"");

        assert_eq!(read(&dir, json!({"path": "a.txt"})), Ok(String::new()));
    }

    #[test]
    fn limit_zero_reads_one_line() {
        let dir = dir_with("a.txt", b"1\n2\n");

        assert_eq!(
            read(&dir, json!({"path": "a.txt", "limit": 0})),
            Ok("1\n[showing lines 1-1 of 2; use offset 2 to read more]\n".into())
        );
    }

    #[test]
    fn a_line_over_the_byte_cap_is_cut_and_skipped() {
        let contents = format!("short\n{}é\nnext\n", "x".repeat(MAX_BYTES - 1));
        let dir = dir_with("a.txt", contents.as_bytes());

        let out = read(&dir, json!({"path": "a.txt", "offset": 2})).unwrap();
        assert_eq!(
            out,
            format!(
                "{}\n[line 2 cut at {} bytes; use offset 3 to read more]\n",
                "x".repeat(MAX_BYTES - 1),
                MAX_BYTES - 1
            )
        );
    }

    #[test]
    fn offset_past_the_end_is_an_error() {
        let dir = dir_with("a.txt", b"1\n");

        assert!(
            read(&dir, json!({"path": "a.txt", "offset": 5}))
                .unwrap_err()
                .contains("past the last line")
        );
    }
}
