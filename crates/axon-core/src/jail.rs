//! Keeps tool paths inside the session's working directory.

use std::path::{Path, PathBuf};

/// Resolve an existing `path` against `cwd` (which must be canonical),
/// following `..` and symlinks, and reject it if it ends up outside `cwd`.
pub(crate) fn resolve(cwd: &Path, path: &str) -> Result<PathBuf, String> {
    let resolved = cwd
        .join(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    if resolved.starts_with(cwd) {
        Ok(resolved)
    } else {
        Err(format!("{path}: outside the working directory"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A canonical working directory inside a temp dir, with a file next to it.
    fn setup() -> (TempDir, PathBuf) {
        let root = TempDir::new().unwrap();
        let cwd = root.path().canonicalize().unwrap().join("cwd");
        fs::create_dir_all(cwd.join("src")).unwrap();
        fs::write(cwd.join("src/main.rs"), "").unwrap();
        fs::write(root.path().join("secret"), "").unwrap();
        (root, cwd)
    }

    #[test]
    fn paths_inside_resolve() {
        let (_root, cwd) = setup();

        assert_eq!(resolve(&cwd, "src/main.rs"), Ok(cwd.join("src/main.rs")));
        assert_eq!(
            resolve(&cwd, "src/../src/main.rs"),
            Ok(cwd.join("src/main.rs"))
        );
        assert_eq!(resolve(&cwd, "."), Ok(cwd.clone()));
    }

    #[test]
    fn dot_dot_and_absolute_escapes_are_rejected() {
        let (root, cwd) = setup();
        let secret = root.path().join("secret");

        assert!(resolve(&cwd, "../secret").unwrap_err().contains("outside"));
        assert!(
            resolve(&cwd, secret.to_str().unwrap())
                .unwrap_err()
                .contains("outside")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let (root, cwd) = setup();
        std::os::unix::fs::symlink(root.path().join("secret"), cwd.join("link")).unwrap();

        assert!(resolve(&cwd, "link").unwrap_err().contains("outside"));
    }

    #[test]
    fn missing_path_is_an_error() {
        let (_root, cwd) = setup();

        assert!(
            resolve(&cwd, "nope.rs")
                .unwrap_err()
                .starts_with("nope.rs: ")
        );
    }
}
