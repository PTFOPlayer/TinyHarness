//! Path-based sandbox: restricts tool file access to a single root directory.
//!
//! Enabled via the CLI `--sandbox` flag. All path-based tools (`ls`, `read`,
//! `write`, `edit`, `grep`, `glob`) are confined to the sandbox root; the
//! `run` tool's explicit `cwd` is likewise checked. Checks fail closed —
//! a violation is reported as a tool error regardless of auto-accept mode.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Normalize a path lexically: resolve `.` and `..` components without
/// touching the filesystem. The input should be absolute; a `..` that would
/// escape the filesystem root is clamped at the root.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // pop() returns false at the root — we stay clamped there.
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Extract the static (non-wildcard) directory prefix of a glob pattern.
///
/// Everything before the first glob metacharacter (`*`, `?`, `[`), trimmed
/// back to the last `/`, is a literal path prefix. Patterns without any `/`
/// before the first metacharacter are anchored at the current directory.
///
/// Examples: `**/*.rs` → `.`, `src/**/*.toml` → `src`, `/etc/**` → `/etc`,
/// `../foo/*.rs` → `../foo`, `/abs/file.txt` → `/abs`.
fn glob_static_prefix(pattern: &str) -> &str {
    let meta_idx = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    let prefix = &pattern[..meta_idx];
    match prefix.rfind('/') {
        Some(i) if i > 0 => &pattern[..i],
        Some(_) => "/", // pattern anchored at filesystem root — static prefix is `/`
        None => ".",
    }
}

/// A sandbox that confines path access to a single root directory.
///
/// The root is canonicalized at construction time (resolving symlinks), and
/// every checked path is resolved against it. Resolution handles:
/// - relative paths (joined with the sandbox root),
/// - `..` traversal (lexical normalization),
/// - symlink escapes (canonicalization of the nearest existing ancestor).
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// Canonicalized absolute root directory.
    root: PathBuf,
}

impl Sandbox {
    /// Create a sandbox rooted at `root`. The directory must exist; it is
    /// canonicalized so all comparisons happen against a symlink-free path.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref();
        let canonical = root.canonicalize().map_err(|e| {
            format!(
                "Failed to canonicalize sandbox root '{}': {}",
                root.display(),
                e
            )
        })?;
        if !canonical.is_dir() {
            return Err(format!(
                "Sandbox root '{}' is not a directory",
                root.display()
            ));
        }
        Ok(Self { root: canonical })
    }

    /// The canonicalized sandbox root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve `raw` (a path as supplied by a tool call) and verify it stays
    /// within the sandbox root.
    ///
    /// Returns `Ok(resolved)` with the fully resolved absolute path on
    /// success, or `Err(message)` describing the violation. This check fails
    /// closed: any path that cannot be proven to be inside the root is
    /// rejected.
    pub fn resolve_contained(&self, raw: &str) -> Result<PathBuf, String> {
        if raw.trim().is_empty() {
            return Err(self.violation_message(raw));
        }

        let candidate = Path::new(raw);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let normalized = normalize_lexical(&joined);

        // Lexical containment check — catches `..` traversal and absolute
        // paths that point elsewhere.
        if !normalized.starts_with(&self.root) {
            return Err(self.violation_message(raw));
        }

        // Symlink containment check: canonicalize the nearest existing
        // ancestor, then re-append the (non-existent) tail. If the ancestor
        // is a symlink pointing outside the sandbox, this detects it.
        let mut ancestor = normalized.as_path();
        let mut tail: Vec<&OsStr> = Vec::new();
        while !ancestor.exists() {
            match (ancestor.file_name(), ancestor.parent()) {
                (Some(name), Some(parent)) if parent != ancestor => {
                    tail.push(name);
                    ancestor = parent;
                }
                _ => break,
            }
        }

        let canonical_ancestor = ancestor
            .canonicalize()
            .map_err(|_| self.violation_message(raw))?;
        let mut resolved = canonical_ancestor;
        for name in tail.iter().rev() {
            resolved.push(name);
        }

        if !resolved.starts_with(&self.root) {
            return Err(self.violation_message(raw));
        }

        Ok(resolved)
    }

    /// Returns `true` if `raw` resolves to a path inside the sandbox root.
    pub fn contains(&self, raw: &str) -> bool {
        self.resolve_contained(raw).is_ok()
    }

    /// Check a glob pattern against the sandbox: the static (non-wildcard)
    /// prefix of the pattern must resolve inside the sandbox root.
    ///
    /// Bare patterns like `**/*.rs` are anchored at the current directory
    /// (the sandbox root) and always pass. Absolute or `..`-escaping prefixes
    /// are rejected.
    pub fn check_glob_pattern(&self, pattern: &str) -> Result<(), String> {
        let base = glob_static_prefix(pattern);
        self.resolve_contained(base).map(|_| ())
    }

    /// Standard error message for a sandbox violation, usable as a tool result.
    pub fn violation_message(&self, raw: &str) -> String {
        format!(
            "Error: Sandbox violation: path '{}' is outside the sandbox root '{}'. \
             Access is restricted to the workspace directory.",
            raw,
            self.root.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lexical_resolves_dotdot() {
        let p = normalize_lexical(Path::new("/a/b/../c/./d"));
        assert_eq!(p, PathBuf::from("/a/c/d"));
    }

    #[test]
    fn normalize_lexical_clamps_at_root() {
        let p = normalize_lexical(Path::new("/a/../../etc/passwd"));
        assert_eq!(p, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn new_requires_existing_dir() {
        assert!(Sandbox::new("/definitely/not/a/real/dir").is_err());
    }

    #[test]
    fn contains_absolute_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        let inside = sb.root().join("file.txt");
        assert!(sb.contains(inside.to_str().unwrap()));
    }

    #[test]
    fn contains_absolute_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        assert!(!sb.contains("/etc/passwd"));
        let other = tempfile::tempdir().unwrap();
        assert!(!sb.contains(other.path().join("x.txt").to_str().unwrap()));
    }

    #[test]
    fn contains_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        assert!(sb.contains("file.txt"));
        assert!(sb.contains("./sub/dir/file.txt"));
        assert!(sb.contains("sub/../other.txt"));
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        assert!(!sb.contains("../escape.txt"));
        assert!(!sb.contains("sub/../../escape.txt"));
        let traversal = format!("{}/../outside.txt", sb.root().display());
        assert!(!sb.contains(&traversal));
    }

    #[test]
    fn allows_nonexistent_paths_inside_root() {
        // write tool creates files that don't exist yet — those must pass.
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        assert!(sb.contains("does/not/exist/yet.txt"));
        let resolved = sb.resolve_contained("new/nested/file.rs").unwrap();
        assert!(resolved.starts_with(sb.root()));
        assert!(resolved.ends_with("new/nested/file.rs"));
    }

    #[test]
    fn rejects_nonexistent_paths_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        assert!(!sb.contains("/tmp/sandbox-evil-dir-xyz/new.txt"));
    }

    #[test]
    fn rejects_symlink_escape() {
        #[cfg(unix)]
        {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let sb = Sandbox::new(tmp.path()).unwrap();

            // Create a symlink inside the sandbox pointing outside.
            let link = tmp.path().join("evil_link");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();

            assert!(!sb.contains("evil_link/secret.txt"));
            assert!(!sb.contains(link.to_str().unwrap()));
        }
    }

    #[test]
    fn violation_message_mentions_path_and_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        let msg = sb.violation_message("/etc/passwd");
        assert!(msg.contains("/etc/passwd"));
        assert!(msg.contains(&sb.root().display().to_string()));
        assert!(msg.starts_with("Error: Sandbox violation"));
    }

    #[test]
    fn rejects_empty_path() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(tmp.path()).unwrap();
        assert!(!sb.contains(""));
        assert!(!sb.contains("   "));
    }
}
