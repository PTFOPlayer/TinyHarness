//! Path containment guard for the built-in file tools.
//!
//! The built-in tools (`ls`, `read`, `write`, `edit`, `grep`, `glob`) run
//! in-process, so they cannot be confined by an OS-level sandbox the way the
//! shell tools can. Their containment is therefore a *path* check performed
//! before the tool handler executes: every path argument must resolve inside
//! the workspace root (or an explicitly allowed extra root).
//!
//! [`PathGuard`] is always active — there is no flag to turn it off. The
//! workspace root is trusted; everything else is unreachable unless the user
//! adds it to `allowed_paths`. Resolution handles:
//!
//! - relative paths (joined with the workspace root),
//! - `..` traversal,
//! - symlink escapes, including a `..` *after* a symlink (resolved component
//!   by component, in the order `open(2)` uses),
//! - glob patterns (only the static prefix can be checked, so `glob` also
//!   filters its results — see [`crate::tools::glob`]).
//!
//! Checks fail closed: a path that cannot be *proven* to be inside an allowed
//! root is rejected, and the rejection is reported as a tool error regardless
//! of auto-accept mode. Callers must act on the **returned** path rather than
//! the raw argument — a relative argument means "relative to the workspace
//! root" here, but "relative to the process cwd" to `std::fs`.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Prefixes that extra allowed roots may never cover.
///
/// The workspace root is exempt from this list (opening TinyHarness inside,
/// say, `/etc` has to keep working); it applies to paths reached through
/// `allowed_paths`, so a too-broad entry like `"/"` cannot be used to write
/// system configuration.
pub const DEFAULT_BLOCKED_PREFIXES: &[&str] = &[
    "/etc", "/usr", "/boot", "/sys", "/proc", "/dev", "/root", "/var/log", "/var/run", "/run",
];

/// Maximum symlink hops tolerated while resolving a path, matching the
/// kernel's `ELOOP` limit. Beyond it the resolution fails closed.
const MAX_SYMLINK_HOPS: usize = 40;

/// Normalize a path lexically: resolve `.` and `..` components without
/// touching the filesystem. The input should be absolute; a `..` that would
/// escape the filesystem root is clamped at the root.
///
/// Only for paths that do not exist yet (configuration prefixes, where there
/// is nothing to canonicalize). Containment of a *tool call* must go through
/// [`PathGuard::resolve_contained`], because lexical `..` and the kernel's
/// left-to-right resolution disagree whenever a symlink is involved:
/// `<root>/link/../etc/passwd` (with `link -> /usr`) normalizes lexically to
/// `<root>/etc/passwd` but the kernel opens `/etc/passwd`.
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

/// Reassemble a component stack produced by [`PathGuard::resolve_kernel_order`].
///
/// The stack always starts with the `"/"` marker, so the result is absolute.
fn join_components(parts: &[OsString]) -> PathBuf {
    let mut path = PathBuf::new();
    for part in parts {
        path.push(part);
    }
    path
}

/// Extract the static (non-wildcard) directory prefix of a glob pattern.
///
/// Everything before the first glob metacharacter (`*`, `?`, `[`), trimmed
/// back to the last `/`, is a literal path prefix. Patterns without any `/`
/// before the first metacharacter are anchored at the current directory.
///
/// Examples: `**/*.rs` → `.`, `src/**/*.toml` → `src`, `/etc/**` → `/etc`,
/// `../foo/*.rs` → `../foo`, `/abs/file.txt` → `/abs`.
pub(crate) fn glob_static_prefix(pattern: &str) -> &str {
    let meta_idx = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    let prefix = &pattern[..meta_idx];
    match prefix.rfind('/') {
        Some(i) if i > 0 => &pattern[..i],
        Some(_) => "/", // pattern anchored at filesystem root — static prefix is `/`
        None => ".",
    }
}

/// Expand a leading `~` using the injected home directory.
///
/// Only `~` and `~/…` are supported — a `~user` form would need a passwd
/// lookup, and an absolute path is the clearer spelling there anyway. Errors
/// when no home directory was injected rather than silently resolving against
/// the workspace root.
fn expand_tilde(raw: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if raw == "~" {
        return home
            .map(|p| p.to_path_buf())
            .ok_or_else(|| format!("allowed_paths entry '{raw}' needs a home directory"));
    }
    let Some(rest) = raw.strip_prefix("~/") else {
        return Err(format!(
            "allowed_paths entry '{raw}' uses an unsupported '~user' form; use '~/…' or an absolute path"
        ));
    };
    home.map(|p| p.join(rest))
        .ok_or_else(|| format!("allowed_paths entry '{raw}' needs a home directory"))
}

/// Builder for [`PathGuard`] that resolves user-configured extra roots.
///
/// Everything is injectable (no ambient `HOME`) to match the [`Paths`]
/// discipline: pass the home directory explicitly when `allowed_paths` may
/// contain `~`.
///
/// [`Paths`]: crate::paths::Paths
#[derive(Debug)]
pub struct PathGuardBuilder {
    root: PathBuf,
    allowed_raw: Vec<String>,
    extra_blocked: Vec<String>,
    blocked_override: Option<Vec<String>>,
    home: Option<PathBuf>,
}

impl PathGuardBuilder {
    /// Add the raw `allowed_paths` entries (absolute, `~`-prefixed, or
    /// relative to the workspace root).
    pub fn allowed_paths<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_raw.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Append prefixes to the default blocked list.
    pub fn extra_blocked_paths<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extra_blocked.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Replace the blocked-prefix list entirely, dropping the defaults.
    ///
    /// Only for callers that need a bespoke policy — the defaults exist to
    /// stop an over-broad `allowed_paths` entry like `"/"` from reaching
    /// system configuration.
    pub fn blocked_paths<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.blocked_override = Some(paths.into_iter().map(Into::into).collect());
        self
    }

    /// The home directory used to expand `~` in configured paths.
    pub fn home(mut self, home: Option<&Path>) -> Self {
        self.home = home.map(|p| p.to_path_buf());
        self
    }

    /// The effective blocked prefixes: the override if given, otherwise the
    /// defaults plus any extras.
    fn effective_blocked(&self) -> Vec<PathBuf> {
        let raw: Vec<String> = match &self.blocked_override {
            Some(list) => list.clone(),
            None => DEFAULT_BLOCKED_PREFIXES
                .iter()
                .map(|s| (*s).to_string())
                .chain(self.extra_blocked.iter().cloned())
                .collect(),
        };
        raw.iter()
            .map(|s| normalize_lexical(Path::new(s)))
            .collect()
    }

    /// Resolve everything and build the guard.
    ///
    /// Returns `Err` for a configuration entry that cannot be resolved (bad
    /// `~` expansion) so a typo in `settings.json` surfaces at startup rather
    /// than as a confusing tool error mid-session. Entries swallowed by a
    /// blocked prefix are dropped with a warning — they're pointless, not
    /// dangerous.
    pub fn build(self) -> Result<PathGuard, String> {
        let canonical_root = self.root.canonicalize().map_err(|e| {
            format!(
                "Failed to canonicalize workspace root '{}': {}",
                self.root.display(),
                e
            )
        })?;
        if !canonical_root.is_dir() {
            return Err(format!(
                "Workspace root '{}' is not a directory",
                self.root.display()
            ));
        }

        let blocked = self.effective_blocked();
        let home = self.home.as_deref();
        let mut allowed = Vec::new();
        for entry in &self.allowed_raw {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            let expanded = if trimmed.starts_with('~') {
                expand_tilde(trimmed, home)
                    .map_err(|e| format!("invalid allowed_paths entry '{entry}': {e}"))?
            } else if Path::new(trimmed).is_absolute() {
                PathBuf::from(trimmed)
            } else {
                canonical_root.join(trimmed)
            };
            let normalized = normalize_lexical(&expanded);
            if blocked.iter().any(|b| normalized.starts_with(b)) {
                tracing::warn!(
                    "allowed_paths entry '{}' is covered by a blocked prefix — ignoring it",
                    entry
                );
                continue;
            }
            allowed.push(normalized);
        }

        Ok(PathGuard {
            root: canonical_root,
            allowed,
            blocked,
        })
    }
}

/// Confines file-tool path access to the workspace root plus any explicitly
/// allowed extra roots.
#[derive(Debug, Clone)]
pub struct PathGuard {
    /// Canonicalized absolute workspace root.
    root: PathBuf,
    /// Extra absolute roots the user allowed (lexically normalized).
    allowed: Vec<PathBuf>,
    /// Prefixes that extra roots may never cover.
    blocked: Vec<PathBuf>,
}

impl PathGuard {
    /// Start building a guard rooted at `root`, with the default blocked
    /// prefixes and no extra allowed roots.
    pub fn builder(root: &Path) -> PathGuardBuilder {
        PathGuardBuilder {
            root: root.to_path_buf(),
            allowed_raw: Vec::new(),
            extra_blocked: Vec::new(),
            blocked_override: None,
            home: None,
        }
    }

    /// Minimal guard with no extra allowed roots — the workspace root only.
    ///
    /// Shorthand for `PathGuard::builder(root).build()`; this is the form used
    /// by tests and by callers with no path configuration.
    pub fn new(root: &Path) -> Result<Self, String> {
        Self::builder(root).build()
    }

    /// Build a guard from already-resolved configuration in one call.
    ///
    /// `allowed` entries follow the same rules as the `allowed_paths` setting:
    /// absolute, `~`-prefixed (expanded with `home`), or relative to `root`.
    /// `extra_blocked` is appended to [`DEFAULT_BLOCKED_PREFIXES`].
    pub fn configured(
        root: &Path,
        allowed: &[String],
        home: Option<&Path>,
        extra_blocked: &[String],
    ) -> Result<Self, String> {
        Self::builder(root)
            .allowed_paths(allowed.iter().cloned())
            .extra_blocked_paths(extra_blocked.iter().cloned())
            .home(home)
            .build()
    }

    /// The canonicalized workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The extra allowed roots (the workspace root is implicit and first).
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed
    }

    /// The blocked prefixes consulted for non-root paths.
    pub fn blocked_prefixes(&self) -> &[PathBuf] {
        &self.blocked
    }

    /// Is `p` (already lexically normalized and absolute) inside an allowed
    /// root?
    ///
    /// The workspace root is trusted outright; extra roots must clear the
    /// blocked list first.
    fn covers(&self, p: &Path) -> bool {
        if p.starts_with(&self.root) {
            return true;
        }
        if self.blocked.iter().any(|b| p.starts_with(b)) {
            return false;
        }
        self.allowed.iter().any(|a| p.starts_with(a))
    }

    /// Resolve `raw` (a path as supplied by a tool call) and verify it stays
    /// within an allowed root.
    ///
    /// Returns `Ok(resolved)` with the fully resolved absolute path on
    /// success, or `Err(message)` describing the violation. Callers must use
    /// the returned path rather than the raw argument: the raw string may be
    /// relative, and a relative path means different things to this guard
    /// (resolved against the workspace root) and to `std::fs` (resolved
    /// against the process cwd).
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

        // Fast path: a purely lexical rejection needs no filesystem access, and
        // it is the common case for an obvious escape (`../x`, `/etc/passwd`).
        let lexical = normalize_lexical(&joined);
        if !self.covers(&lexical) {
            return Err(self.violation_message(raw));
        }

        // Real resolution, in the order the kernel uses. The lexical check
        // above is *not* sufficient: `<root>/link/../etc/passwd` with
        // `link -> /usr` normalizes to `<root>/etc/passwd` (inside the root)
        // but opens `/etc/passwd`.
        let resolved = self
            .resolve_kernel_order(&joined)
            .ok_or_else(|| self.violation_message(raw))?;

        if !self.covers(&resolved) {
            return Err(self.violation_message(raw));
        }

        Ok(resolved)
    }

    /// Walk `joined` component by component, following symlinks as they are
    /// encountered, so a `..` applies to the *link's target* rather than to
    /// the textual path — the semantics `open(2)` uses.
    ///
    /// Components that do not exist yet are appended literally and the walk
    /// continues (a `write` legitimately names a new file; its not-yet-existing
    /// parents cannot be symlinks). Exceeding [`MAX_SYMLINK_HOPS`], or a
    /// symlink that cannot be read, returns `None` — callers fail closed.
    fn resolve_kernel_order(&self, joined: &Path) -> Option<PathBuf> {
        debug_assert!(joined.is_absolute());

        // The resolved component stack. Always begins with the root marker.
        let mut out: Vec<OsString> = vec![OsString::from("/")];
        // Components still to process; symlink targets are spliced at the front.
        let mut rest: VecDeque<OsString> = joined
            .components()
            .filter(|c| !matches!(c, Component::RootDir))
            .map(|c| OsString::from(c.as_os_str()))
            .collect();

        let mut hops = 0;
        while let Some(comp) = rest.pop_front() {
            match comp.to_str() {
                Some(".") => continue,
                Some("..") => {
                    // Pop, but never past the root marker.
                    if out.len() > 1 {
                        out.pop();
                    }
                    continue;
                }
                _ => {}
            }

            out.push(comp);
            let candidate = join_components(&out);
            let meta = match std::fs::symlink_metadata(&candidate) {
                Ok(meta) => meta,
                // Missing: nothing below it can exist either, so the remaining
                // components are appended literally by subsequent iterations.
                Err(_) => continue,
            };
            if !meta.file_type().is_symlink() {
                continue;
            }

            hops += 1;
            if hops > MAX_SYMLINK_HOPS {
                return None;
            }
            let target = std::fs::read_link(&candidate).ok()?;
            out.pop();
            if target.is_absolute() {
                out.clear();
                out.push(OsString::from("/"));
            }
            let splice = target
                .components()
                .filter(|c| !matches!(c, Component::RootDir))
                .map(|c| OsString::from(c.as_os_str()))
                .collect::<Vec<_>>();
            for item in splice.into_iter().rev() {
                rest.push_front(item);
            }
        }

        Some(join_components(&out))
    }

    /// Returns `true` if `raw` resolves to a path inside an allowed root.
    pub fn contains(&self, raw: &str) -> bool {
        self.resolve_contained(raw).is_ok()
    }

    /// Check a glob pattern against the guard: the static (non-wildcard)
    /// prefix of the pattern must resolve inside an allowed root.
    ///
    /// Bare patterns like `**/*.rs` are anchored at the workspace root and
    /// always pass. Absolute or `..`-escaping prefixes are rejected.
    pub fn check_glob_pattern(&self, pattern: &str) -> Result<(), String> {
        let base = glob_static_prefix(pattern);
        self.resolve_contained(base).map(|_| ())
    }

    /// Standard error message for a violation, usable as a tool result.
    pub fn violation_message(&self, raw: &str) -> String {
        let mut msg = format!(
            "Error: Path '{}' is outside the workspace root '{}'. \
             File-tool access is confined to the workspace",
            raw,
            self.root.display()
        );
        if self.allowed.is_empty() {
            msg.push_str(". Add directories to `allowed_paths` in settings.json to widen it.");
        } else {
            let roots: Vec<String> = self
                .allowed
                .iter()
                .map(|p| format!("'{}'", p.display()))
                .collect();
            msg.push_str(&format!(" (also allowed: {}).", roots.join(", ")));
        }
        msg
    }
}

// The guard is pure path arithmetic, but its symlink tests need POSIX
// semantics (Windows has 8.3 short names and `\\?\` verbatim prefixes).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn guard_in(tmp: &Path) -> PathGuard {
        PathGuard::new(tmp).unwrap()
    }

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
        assert!(PathGuard::new(Path::new("/definitely/not/a/real/dir")).is_err());
    }

    #[test]
    fn contains_absolute_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        let inside = sb.root().join("file.txt");
        assert!(sb.contains(inside.to_str().unwrap()));
    }

    #[test]
    fn contains_absolute_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(!sb.contains("/etc/passwd"));
        let other = tempfile::tempdir().unwrap();
        assert!(!sb.contains(other.path().join("x.txt").to_str().unwrap()));
    }

    #[test]
    fn contains_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(sb.contains("file.txt"));
        assert!(sb.contains("./sub/dir/file.txt"));
        assert!(sb.contains("sub/../other.txt"));
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(!sb.contains("../escape.txt"));
        assert!(!sb.contains("sub/../../escape.txt"));
        let traversal = format!("{}/../outside.txt", sb.root().display());
        assert!(!sb.contains(&traversal));
    }

    #[test]
    fn allows_nonexistent_paths_inside_root() {
        // write tool creates files that don't exist yet — those must pass.
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(sb.contains("does/not/exist/yet.txt"));
        let resolved = sb.resolve_contained("new/nested/file.rs").unwrap();
        assert!(resolved.starts_with(sb.root()));
        assert!(resolved.ends_with("new/nested/file.rs"));
    }

    #[test]
    fn rejects_nonexistent_paths_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(!sb.contains("/tmp/path-guard-evil-dir-xyz/new.txt"));
    }

    #[test]
    fn rejects_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());

        // Create a symlink inside the workspace pointing outside.
        let link = tmp.path().join("evil_link");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        assert!(!sb.contains("evil_link/secret.txt"));
        assert!(!sb.contains(link.to_str().unwrap()));
    }

    #[test]
    fn violation_message_mentions_path_and_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        let msg = sb.violation_message("/etc/passwd");
        assert!(msg.contains("/etc/passwd"));
        assert!(msg.contains(&sb.root().display().to_string()));
        assert!(msg.starts_with("Error: Path '"));
    }

    #[test]
    fn rejects_empty_path() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(!sb.contains(""));
        assert!(!sb.contains("   "));
    }

    #[test]
    fn allowed_paths_widen_the_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let sb = PathGuard::configured(
            tmp.path(),
            &[extra.path().to_string_lossy().to_string()],
            None,
            &[],
        )
        .unwrap();
        assert!(sb.contains(extra.path().join("file.txt").to_str().unwrap()));
        // A sibling of the allowed root is still out of bounds.
        assert!(!sb.contains("/etc/hosts"));
    }

    #[test]
    fn relative_allowed_path_is_joined_with_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("shared")).unwrap();
        let sb = PathGuard::configured(tmp.path(), &["shared".to_string()], None, &[]).unwrap();
        assert!(sb.contains("shared/a.txt"));
        assert!(sb.contains(tmp.path().join("shared/b.txt").to_str().unwrap()));
    }

    #[test]
    fn tilde_allowed_path_needs_home() {
        let tmp = tempfile::tempdir().unwrap();
        // No home injected → `~` entry is a configuration error, not a silent
        // resolution to the workspace root.
        let err =
            PathGuard::configured(tmp.path(), &["~/.cargo".to_string()], None, &[]).unwrap_err();
        assert!(err.contains("home directory"), "{err}");

        let home = tempfile::tempdir().unwrap();
        let sb = PathGuard::configured(
            tmp.path(),
            &["~/.cargo".to_string()],
            Some(home.path()),
            &[],
        )
        .unwrap();
        let cargo = home.path().join(".cargo");
        assert!(sb.contains(cargo.join("registry/index").to_str().unwrap()));
    }

    #[test]
    fn blocked_prefix_defeats_overbroad_allowed_root() {
        let tmp = tempfile::tempdir().unwrap();
        // Allowing "/" must not make /etc writable — the blocked list wins.
        let sb = PathGuard::configured(tmp.path(), &["/".to_string()], None, &[]).unwrap();
        assert!(!sb.contains("/etc/passwd"));
        assert!(!sb.contains("/usr/lib/libc.so"));
        // ...but an unrelated path under "/" is reachable.
        let other = tempfile::tempdir().unwrap();
        assert!(sb.contains(other.path().join("x.txt").to_str().unwrap()));
    }

    #[test]
    fn workspace_root_is_exempt_from_blocked_prefixes() {
        // Opening the harness inside a blocked prefix still has to work.
        let tmp = tempfile::tempdir().unwrap();
        let sb = PathGuard::configured(tmp.path(), &["/".to_string()], None, &[]).unwrap();
        assert!(sb.contains("inside-me/etc/passwd"));
    }

    #[test]
    fn overbroad_allowed_entry_is_reported_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        // `/etc` is blocked, so the entry is dropped with a warning; the guard
        // still builds and still confines everything else.
        let sb = PathGuard::configured(tmp.path(), &["/etc".to_string()], None, &[]).unwrap();
        assert!(sb.allowed_roots().is_empty());
        assert!(!sb.contains("/etc/passwd"));
    }

    #[test]
    fn glob_pattern_uses_static_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = guard_in(tmp.path());
        assert!(sb.check_glob_pattern("**/*.rs").is_ok());
        assert!(sb.check_glob_pattern("src/**/*.toml").is_ok());
        assert!(sb.check_glob_pattern("/etc/**").is_err());
        assert!(sb.check_glob_pattern("../secret/*.txt").is_err());
    }

    /// The `link -> /usr` + `..` case: lexical normalization collapses
    /// `<root>/link/../etc/passwd` to `<root>/etc/passwd`, which is *inside*
    /// the root, while the kernel opens `/etc/passwd`. Resolution must follow
    /// the kernel's order, not the textual one.
    #[test]
    fn rejects_symlink_then_dotdot_reaching_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(Path::new("/usr"), &link).unwrap();
        let sb = guard_in(tmp.path());

        assert!(!sb.contains("link/../etc/passwd"));
        assert!(!sb.contains(&format!("{}/link/../etc/passwd", tmp.path().display())));
        assert!(!sb.contains("link/../outside_me.txt"));
    }

    /// Same shape, but the symlink points at a *sibling* directory rather than
    /// a system path — the escape is the traversal, not the target.
    #[test]
    fn rejects_symlink_then_dotdot_into_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().parent().unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(parent, &link).unwrap();
        let sb = guard_in(tmp.path());

        // `<root>/link/..` is `<root>` lexically, but the kernel resolves the
        // link first, making it the workspace's *parent*.
        assert!(!sb.contains("link/../sibling-secret.txt"));
    }

    /// A single in-workspace symlink must not defeat `DEFAULT_BLOCKED_PREFIXES`
    /// by way of `..` — the blocked list is the last line of defence for an
    /// over-broad `allowed_paths`, so it has to hold here too.
    #[test]
    fn symlink_dotdot_cannot_reach_a_blocked_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(Path::new("/usr"), tmp.path().join("link")).unwrap();
        let sb = PathGuard::configured(tmp.path(), &["/".to_string()], None, &[]).unwrap();

        assert!(!sb.contains("link/../etc/passwd"));
        assert!(!sb.contains("link/../usr/lib"));
    }

    /// Chains of relative symlinks resolve in the same order the kernel uses.
    #[test]
    fn follows_relative_symlink_chains_in_kernel_order() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("b")).unwrap();
        std::fs::write(tmp.path().join("b/target.txt"), "x").unwrap();
        // a/one -> ../b, i.e. a *relative* link: its meaning depends on the
        // directory it is reached through, so only kernel-order resolution can
        // place it correctly.
        std::os::unix::fs::symlink(Path::new("../b"), tmp.path().join("a/one")).unwrap();
        let sb = guard_in(tmp.path());

        // a/one resolves to <root>/b, so the target is reachable and in bounds.
        assert!(sb.contains("a/one/target.txt"));
        // a/one/.. is <root> for the kernel (the link is resolved first) but
        // <root>/a for a lexical collapse — the two disagree, and the kernel
        // is the one that will open the file.
        let resolved = sb.resolve_contained("a/one/../target.txt").unwrap();
        assert_eq!(resolved, sb.root().join("target.txt"));
    }

    /// A symlink loop must fail closed rather than hang or accept.
    #[test]
    fn rejects_symlink_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let sb = guard_in(tmp.path());

        assert!(!sb.contains("a/x.txt"));
        assert!(!sb.contains("b/x.txt"));
    }

    #[test]
    fn glob_pattern_honours_allowed_root() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let sb = PathGuard::configured(
            tmp.path(),
            &[extra.path().to_string_lossy().to_string()],
            None,
            &[],
        )
        .unwrap();
        let pattern = format!("{}/**/*.rs", extra.path().display());
        assert!(sb.check_glob_pattern(&pattern).is_ok());
    }
}
