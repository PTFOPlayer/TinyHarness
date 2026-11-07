//! OS-level process sandbox for shell-executing tools (`run`, custom tools).
//!
//! The built-in file tools are confined by [`PathGuard`](crate::path_guard::PathGuard)
//! — a pure path check, because they run in-process. Shell commands cannot be
//! confined that way: a command string is arbitrary, so there is nothing
//! reliable to inspect before it runs. Those go through a real kernel-enforced
//! sandbox instead, via `bwrap` (bubblewrap) or `firejail`.
//!
//! Both backends are auto-detected ([`Sandbox::detect`]); `bwrap` is preferred
//! because it is deny-by-default and needs no setuid helper.
//!
//! # What the backends guarantee
//!
//! **bwrap** builds a fresh mount tree from a `tmpfs` root and binds *only*
//! what the command needs, so anything not bound simply does not exist inside
//! the sandbox: the workspace (read-write), the system libraries and `/etc`
//! (read-only), the file `/etc/resolv.conf` points at when it is a symlink out
//! of `/etc` (see `resolv_conf_target`), and whatever the user added via
//! `allowed_paths`. It also unshares the PID, IPC, UTS and cgroup namespaces,
//! clears the environment, and starts a new session so a command cannot grab
//! the terminal.
//!
//! **firejail** takes the opposite approach: it mounts the *existing* host
//! filesystem read-only and re-binds the workspace read-write, then drops
//! capabilities and applies seccomp. Reads outside the workspace are
//! therefore **still possible** under firejail — it is write/privilege
//! containment, not visibility containment. It also normally relies on a
//! setuid helper. Prefer bwrap when containment matters.
//!
//! # Network
//!
//! The network is **always available** to sandboxed commands, under both
//! backends. A coding agent runs `git fetch`, `curl`, and `cargo add` constantly;
//! blocking those would make the sandbox unusable rather than safe, and the
//! exfiltration risk it would mitigate is already accepted by giving the model
//! shell access. Network isolation is out of scope here.
//!
//! Reachability is not the same as *configuration*, though: name resolution
//! needs `/etc/resolv.conf`, which on a systemd-resolved host is a symlink into
//! the unbound `/run`. bwrap therefore binds that target explicitly, or every
//! `curl`/`npm`/`git fetch` fails with a DNS error that looks exactly like a
//! blocked network.
//!
//! # Limits
//!
//! Neither backend defends against a hostile *user*: the sandboxed process
//! still runs as the same uid, so anything the workspace bind exposes (`.env`,
//! `.git`, committed credentials) is in scope — and reachable over the network.
//! Nor is either a container: no image, no uid remapping, no resource limits
//! beyond what the flags here provide.
//!
//! # Widening the sandbox
//!
//! `allowed_paths` widens the filesystem view (read-only) and
//! `sandbox.writable_paths` grants write access to specific extra directories.
//! Both are reported in the startup banner.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

/// Which OS facility provides the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    /// bubblewrap — unprivileged user-namespace sandbox (preferred).
    Bwrap,
    /// firejail — setuid sandbox with a read-only host root.
    Firejail,
}

/// Every backend, in preference order.
pub const ALL_BACKENDS: [SandboxBackend; 2] = [SandboxBackend::Bwrap, SandboxBackend::Firejail];

impl SandboxBackend {
    /// The executable that implements this backend.
    pub fn binary(self) -> &'static str {
        match self {
            SandboxBackend::Bwrap => "bwrap",
            SandboxBackend::Firejail => "firejail",
        }
    }

    /// Parse a configuration value naming a specific backend.
    ///
    /// `auto` and `off` select a *policy*, not a backend, so the caller
    /// handles them via [`Self::is_auto`] / [`Self::is_off`] and this returns
    /// `None` for both.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bwrap" | "bubblewrap" => Some(SandboxBackend::Bwrap),
            "firejail" | "fj" => Some(SandboxBackend::Firejail),
            _ => None,
        }
    }

    /// `true` for the values that mean "no sandbox".
    pub fn is_off(value: &str) -> bool {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "none" | "no" | "false" | "0" | "disabled"
        )
    }

    /// `true` for the values that mean "pick whatever is installed".
    pub fn is_auto(value: &str) -> bool {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "auto" | "detect" | "yes" | "true" | "1" | "enabled"
        )
    }
}

impl std::fmt::Display for SandboxBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.binary())
    }
}

/// Host paths and tunables a [`Sandbox`] binds into the sandbox.
///
/// Everything here is resolved by the caller from settings and the host
/// environment, so this module never reads `$HOME` itself — consistent with
/// the [`Paths`](crate::paths::Paths) discipline that owns ambient lookups.
#[derive(Debug, Clone, Default)]
pub struct SandboxOptions {
    /// Extra host paths bound read-only (from `allowed_paths`).
    pub read_only_paths: Vec<PathBuf>,
    /// Extra host paths bound read-write (from `allowed_paths`).
    pub read_write_paths: Vec<PathBuf>,
    /// Host `$HOME`, bound read-only so paths nested under it stay reachable.
    pub home: Option<PathBuf>,
    /// Writable directory exposed as `$CARGO_HOME`. A read-only toolchain
    /// cannot update cargo's package-cache lock, so builds need *somewhere*
    /// writable; pointing this at a cache directory under the data dir keeps
    /// `~/.cargo` untouched at the cost of an empty registry.
    pub cargo_home: Option<PathBuf>,
    /// Host `$RUSTUP_HOME`, bound read-only so a rustup proxy resolves a
    /// toolchain.
    pub rustup_home: Option<PathBuf>,
    /// Host `$CARGO_HOME/bin`, bound read-only so `cargo`/`rustc` are on
    /// `PATH`. Separate from [`Self::cargo_home`] because that one must be
    /// writable.
    pub cargo_bin: Option<PathBuf>,
}

impl SandboxOptions {
    /// The home directory, exposed to sandboxed commands only as `$HOME`.
    ///
    /// Never bound: bwrap creates mountpoint parents on demand, so a nested
    /// read-only bind (say `~/.cargo/bin`) works without exposing the rest of
    /// home — and binding home would defeat the point of the tmpfs root by
    /// making every personal directory under it visible.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }
}

/// A configured OS-level sandbox for one workspace.
///
/// Cheap to clone, safe to share: host layout is resolved at construction and
/// [`Sandbox::argv`] only assembles vectors.
#[derive(Debug, Clone)]
pub struct Sandbox {
    backend: SandboxBackend,
    workspace: PathBuf,
    options: SandboxOptions,
    /// `/lib`-style symlinks that must be recreated inside the bwrap mount
    /// tree (empty for firejail, which sees the host tree).
    symlinks: Vec<(PathBuf, PathBuf)>,
    /// The real file behind `/etc/resolv.conf` when it is a symlink pointing
    /// outside `/etc` (empty for firejail, which sees the host tree).
    ///
    /// See [`resolv_conf_target`].
    resolv_conf: Option<PathBuf>,
}

/// The host's `/etc/resolv.conf` target, if it is a symlink leaving `/etc`.
///
/// bwrap binds `/etc` read-only but deliberately not `/run`, so on a
/// systemd-resolved host — where `/etc/resolv.conf` is a symlink to
/// `/run/systemd/resolve/stub-resolv.conf` — the link *dangles* inside the
/// sandbox and every name resolution fails with `EAI_NONAME` / curl exit 6.
/// That looks exactly like "the sandbox blocked the network", and it is not:
/// the network is wide open. Binding the resolved target at its own path
/// restores the link.
///
/// Returns `None` when `/etc/resolv.conf` is a regular file (already covered
/// by the `/etc` bind) or unreadable, so a non-systemd host changes nothing.
fn resolv_conf_target_at(link_path: &Path) -> Option<PathBuf> {
    if !std::fs::symlink_metadata(link_path)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let target = std::fs::read_link(link_path).ok()?;
    // Relative targets (`-> ../run/...`) resolve against the link's directory,
    // which is what the sandbox also sees, so canonicalize against `/etc`.
    let absolute = if target.is_absolute() {
        target
    } else {
        link_path.parent()?.join(target)
    };
    let resolved = normalize_existing(&absolute)?;
    // A target still inside `/etc` needs no extra bind.
    let etc = link_path.parent()?;
    if resolved.starts_with(etc) || !resolved.is_file() {
        return None;
    }
    Some(resolved)
}

/// The host's `/etc/resolv.conf` target, if it needs an extra bind.
fn resolv_conf_target() -> Option<PathBuf> {
    resolv_conf_target_at(Path::new("/etc/resolv.conf"))
}

/// Canonicalize `path`, but only if it exists — `None` otherwise.
fn normalize_existing(path: &Path) -> Option<PathBuf> {
    path.exists().then(|| path.canonicalize().ok()).flatten()
}

/// Is `path` a symlink whose target is exactly `target`?
fn is_symlink_to(path: &Path, target: &str) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
        && std::fs::read_link(path)
            .map(|t| t == Path::new(target))
            .unwrap_or(false)
}

impl Sandbox {
    /// Build a sandbox for `workspace` using `backend`.
    ///
    /// The workspace is canonicalized so the bind target and `--chdir` value
    /// agree with what tools see.
    pub fn new(
        backend: SandboxBackend,
        workspace: &Path,
        options: SandboxOptions,
    ) -> Result<Self, String> {
        let canonical = workspace.canonicalize().map_err(|e| {
            format!(
                "Failed to canonicalize sandbox workspace '{}': {}",
                workspace.display(),
                e
            )
        })?;
        if !canonical.is_dir() {
            return Err(format!(
                "Sandbox workspace '{}' is not a directory",
                workspace.display()
            ));
        }

        // bwrap builds its own mount tree, so the merged-usr symlinks the host
        // relies on (`/bin -> usr/bin`, `/lib64 -> usr/lib64`, …) have to be
        // recreated inside it or the dynamic loader and `exec` stop working.
        let symlinks = match backend {
            SandboxBackend::Bwrap => [
                ("/bin", "usr/bin"),
                ("/sbin", "usr/sbin"),
                ("/lib", "usr/lib"),
                ("/lib64", "usr/lib64"),
            ]
            .iter()
            .filter(|(link, target)| is_symlink_to(Path::new(link), target))
            .map(|(link, target)| (PathBuf::from(*link), PathBuf::from(*target)))
            .collect(),
            SandboxBackend::Firejail => Vec::new(),
        };

        let resolv_conf = match backend {
            SandboxBackend::Bwrap => resolv_conf_target(),
            SandboxBackend::Firejail => None,
        };

        Ok(Self {
            backend,
            workspace: canonical,
            options,
            symlinks,
            resolv_conf,
        })
    }

    /// The backend implementing this sandbox.
    pub fn backend(&self) -> SandboxBackend {
        self.backend
    }

    /// The canonicalized workspace root, exposed read-write inside the sandbox.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The options this sandbox was built with.
    pub fn options(&self) -> &SandboxOptions {
        &self.options
    }

    /// Locate `backend`'s binary on `PATH`.
    ///
    /// Deliberately does not shell out to `which`: a lookup that only touches
    /// `PATH` works everywhere without spawning anything.
    pub fn locate(backend: SandboxBackend) -> Option<PathBuf> {
        let exe = format!("{}{}", backend.binary(), std::env::consts::EXE_SUFFIX);
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(&exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Every backend whose binary is installed, in preference order.
    pub fn available() -> Vec<SandboxBackend> {
        ALL_BACKENDS
            .into_iter()
            .filter(|b| Self::locate(*b).is_some())
            .collect()
    }

    /// Build a sandbox on the best installed backend (bwrap first).
    ///
    /// The `Err` explains that neither is available and how to get one; the
    /// caller turns it into a startup message rather than a hard failure, so a
    /// machine without sandbox tooling still starts.
    pub fn detect(workspace: &Path, options: SandboxOptions) -> Result<Self, String> {
        match Self::available().first().copied() {
            Some(backend) => Self::new(backend, workspace, options),
            None => Err(
                "No sandbox backend found. Install bubblewrap (`bwrap`) or firejail, \
                 or start with --no-sandbox to run shell commands unsandboxed."
                    .to_string(),
            ),
        }
    }

    /// Resolve a backend name (from `--sandbox=<value>` or `sandbox.backend`)
    /// into a sandbox.
    ///
    /// `auto`/empty → [`Sandbox::detect`]; an explicit name → that backend,
    /// with an actionable error if it is not installed.
    pub fn from_setting(
        value: &str,
        workspace: &Path,
        options: SandboxOptions,
    ) -> Result<Self, String> {
        if SandboxBackend::is_off(value) {
            return Err(format!(
                "sandbox backend '{value}' disables sandboxing — nothing to build"
            ));
        }
        if SandboxBackend::is_auto(value) {
            return Self::detect(workspace, options);
        }
        let backend = SandboxBackend::parse(value).ok_or_else(|| {
            format!("Unknown sandbox backend '{value}'. Valid values: auto, bwrap, firejail, off")
        })?;
        if Self::locate(backend).is_none() {
            return Err(format!(
                "'{backend}' is not installed or not on PATH. \
                 Available: {}.",
                if Self::available().is_empty() {
                    "none".to_string()
                } else {
                    Self::available()
                        .into_iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ));
        }
        Self::new(backend, workspace, options)
    }

    /// Assemble the full argv that runs `command` through `/bin/sh` inside
    /// this sandbox.
    ///
    /// Returns `(program, args)`. `cwd` becomes the sandbox's `--chdir` value
    /// when given; the caller must have already established that it is inside
    /// the workspace, because an out-of-tree directory is simply not visible
    /// in the sandbox mount tree and the command would fail confusingly.
    pub fn argv(&self, command: &str, cwd: Option<&Path>) -> (PathBuf, Vec<OsString>) {
        self.argv_with_env(command, cwd, &[])
    }

    /// [`Sandbox::argv`] with extra environment variables for the sandboxed
    /// process.
    ///
    /// Needed because bwrap runs `--clearenv`: variables set on the *outer*
    /// bwrap process (custom tools' `TH_*` parameters, for instance) do not
    /// reach the sandboxed shell, so they have to travel as `--setenv`
    /// arguments. firejail inherits the parent environment, where `extra` is
    /// ignored — the caller should still set it on the spawned command so both
    /// backends behave the same.
    pub fn argv_with_env(
        &self,
        command: &str,
        cwd: Option<&Path>,
        extra: &[(String, String)],
    ) -> (PathBuf, Vec<OsString>) {
        let program =
            Self::locate(self.backend).unwrap_or_else(|| PathBuf::from(self.backend.binary()));
        let mut args: Vec<OsString> = match self.backend {
            SandboxBackend::Bwrap => self.bwrap_args(cwd, extra),
            SandboxBackend::Firejail => self.firejail_args(),
        };
        args.push(OsString::from("/bin/sh"));
        args.push(OsString::from("-c"));
        args.push(OsString::from(command));
        (program, args)
    }

    fn bwrap_args(&self, cwd: Option<&Path>, extra_env: &[(String, String)]) -> Vec<OsString> {
        let mut a: Vec<OsString> = vec![
            // Namespaces. `--unshare-user` is what makes all of this work
            // without root; the rest isolate process/identity views. The
            // network namespace is deliberately *not* unshared — see the
            // module docs.
            os("--unshare-user"),
            os("--unshare-ipc"),
            os("--unshare-pid"),
            os("--unshare-uts"),
            os("--unshare-cgroup"),
            // Exit when TinyHarness goes away, and take a new session so a
            // command cannot seize the controlling terminal.
            os("--die-with-parent"),
            os("--new-session"),
            // Strip the inherited environment: API keys and tokens must not
            // reach a sandboxed command. Only what's needed is re-added below.
            os("--clearenv"),
            // Deny-by-default filesystem: a tmpfs root, then explicit binds.
            // (Creating a mountpoint on a read-only `/` is impossible — bwrap
            // reports "Can't create file …: Read-only file system" — so the
            // tmpfs root is not a stylistic choice.)
            os("--tmpfs"),
            os("/"),
            os("--tmpfs"),
            os("/tmp"),
            // A fresh procfs: `--ro-bind /proc /proc` would leak the host's
            // entire process list into the sandbox.
            os("--proc"),
            os("/proc"),
            // devtmpfs only — no host block devices, so /dev/sda-style reads
            // fail outright.
            os("--dev"),
            os("/dev"),
        ];

        // No `--unshare-net`: sandboxed commands keep the host network so
        // `git`, `curl`, and `cargo add` work. The containment offered here is
        // filesystem and environment isolation; network access is not part of
        // the threat model this sandbox addresses.

        // System toolchain and config, read-only.
        a.extend(bind_ro(Path::new("/usr")));
        a.extend(bind_ro(Path::new("/etc")));
        // `/etc/resolv.conf` is usually a symlink into `/run`, which is not
        // bound — without its target the link dangles and DNS fails even
        // though the network itself is open. `--ro-bind-try` so a target that
        // vanished between construction and execution degrades to no DNS
        // rather than to a sandbox that refuses to start.
        if let Some(resolv) = self.resolv_conf.as_deref() {
            a.push(os("--ro-bind-try"));
            a.push(resolv.as_os_str().to_owned());
            a.push(resolv.as_os_str().to_owned());
        }
        for (link, target) in &self.symlinks {
            a.push(os("--symlink"));
            a.push(target.as_os_str().to_owned());
            a.push(link.as_os_str().to_owned());
        }

        // Rust toolchain: read-only install, writable $CARGO_HOME.
        if let Some(bin) = self.options.cargo_bin.as_deref() {
            a.extend(bind_ro(bin));
        }
        if let Some(rustup) = self.options.rustup_home.as_deref() {
            a.extend(bind_ro(rustup));
        }
        if let Some(cargo_home) = self.options.cargo_home.as_deref() {
            a.extend(bind_rw(cargo_home));
        }

        // User-selected extra paths. bwrap creates mountpoint parents on
        // demand, so a nested path (e.g. `~/.cargo/bin`) binds fine without
        // exposing the rest of `$HOME`.
        for p in &self.options.read_only_paths {
            a.extend(bind_ro(p));
        }
        for p in &self.options.read_write_paths {
            a.extend(bind_rw(p));
        }

        // The workspace itself, read-write, last so it wins over any
        // read-only ancestor bind.
        a.extend(bind_rw(&self.workspace));

        for (key, value) in self.environment() {
            a.push(os("--setenv"));
            a.push(os(&key));
            a.push(OsString::from(value));
        }
        for (key, value) in extra_env {
            a.push(os("--setenv"));
            a.push(os(key));
            a.push(OsString::from(value));
        }

        // An explicit `cwd` overrides the workspace root; both are bound
        // inside the sandbox, so either is a valid `--chdir` target. A
        // relative `cwd` is resolved against the workspace, because bwrap
        // would otherwise resolve it against the sandbox's tmpfs root.
        a.push(os("--chdir"));
        a.push(match cwd {
            Some(dir) if dir.is_absolute() => OsString::from(dir.as_os_str()),
            Some(dir) => OsString::from(self.workspace.join(dir).as_os_str()),
            None => self.workspace.as_os_str().to_owned(),
        });

        a
    }

    fn firejail_args(&self) -> Vec<OsString> {
        // firejail sees the whole host filesystem; `--read-only /` makes it
        // immutable and the workspace is re-bound writable. Unlike bwrap this
        // does *not* hide files outside the workspace from reads.
        let mut a: Vec<OsString> = vec![
            os("--noprofile"),
            os("--quiet"),
            os("--read-only"),
            os("/"),
            os(&format!("--bind={}", self.workspace.display())),
            // Privilege reduction.
            os("--caps.drop=all"),
            os("--nonewprivs"),
            os("--nogroups"),
            os("--seccomp"),
            os("--disable-mnt"),
        ];
        // No `--net=none`: see the note in `bwrap_args` — network access is
        // intentionally preserved for both backends.
        for p in &self.options.read_only_paths {
            if is_usable_dir(p) {
                a.push(os(&format!("--bind-ro={}", p.display())));
            }
        }
        for p in &self.options.read_write_paths {
            if is_usable_dir(p) {
                a.push(os(&format!("--bind={}", p.display())));
            }
        }
        a
    }

    /// The environment re-injected into a bwrap sandbox.
    ///
    /// Only what a build/test command needs: nothing secret is inherited, and
    /// the locale is forwarded so command output matches the host language.
    fn environment(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();

        let host_path = std::env::var_os("PATH")
            .map(PathBuf::from)
            .unwrap_or_default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(bin) = self
            .options
            .cargo_bin
            .as_deref()
            .filter(|p| is_usable_dir(p))
        {
            dirs.push(bin.to_path_buf());
        }
        dirs.extend(std::env::split_paths(&host_path));
        dirs.push(PathBuf::from("/usr/bin"));
        dirs.push(PathBuf::from("/bin"));
        out.push((
            "PATH".to_string(),
            std::env::join_paths(&dirs)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        ));

        // Only advertise the toolchain homes that are actually bound. An
        // env var pointing at a path the sandbox cannot see makes a tool try
        // to create it (bwrap's tmpfs root is writable, so that silently
        // succeeds into an ephemeral directory) instead of failing clearly.
        for (key, value) in [
            ("HOME", self.options.home.as_deref()),
            ("CARGO_HOME", self.options.cargo_home.as_deref()),
            ("RUSTUP_HOME", self.options.rustup_home.as_deref()),
        ] {
            if let Some(v) = value.filter(|p| is_usable_dir(p)) {
                out.push((key.to_string(), v.to_string_lossy().into_owned()));
            }
        }
        for key in ["LANG", "LC_ALL", "LC_CTYPE", "TERM"] {
            if let Some(value) = std::env::var_os(key) {
                out.push((key.to_string(), value.to_string_lossy().into_owned()));
            }
        }
        out
    }

    /// Quick self-check: run `true` through the sandbox.
    ///
    /// Used at startup so a broken backend (missing setuid bit on firejail,
    /// user namespaces disabled by the distro) is reported before the first
    /// tool call rather than as a mysterious command failure.
    pub fn probe(&self) -> Result<(), String> {
        let (program, args) = self.argv("true", None);
        let output = StdCommand::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("Failed to run {}: {e}", program.display()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{} exited with {}: {}",
                program.display(),
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// `OsString` shorthand — the argv builders are unreadable otherwise.
fn os(value: &str) -> OsString {
    OsString::from(value)
}

/// A bind target is only useful if it is a real directory; a missing or
/// regular-file entry would make bwrap fail the whole sandbox.
fn is_usable_dir(path: &Path) -> bool {
    path.is_dir()
}

fn bind_ro(path: &Path) -> Vec<OsString> {
    if !is_usable_dir(path) {
        return Vec::new();
    }
    vec![
        os("--ro-bind"),
        path.as_os_str().to_owned(),
        path.as_os_str().to_owned(),
    ]
}

fn bind_rw(path: &Path) -> Vec<OsString> {
    if !is_usable_dir(path) {
        return Vec::new();
    }
    vec![
        os("--bind"),
        path.as_os_str().to_owned(),
        path.as_os_str().to_owned(),
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sandbox_in(tmp: &Path, options: SandboxOptions) -> Sandbox {
        Sandbox::new(SandboxBackend::Bwrap, tmp, options).unwrap()
    }

    #[test]
    fn backend_parsing() {
        assert_eq!(SandboxBackend::parse("bwrap"), Some(SandboxBackend::Bwrap));
        assert_eq!(
            SandboxBackend::parse("Bubblewrap"),
            Some(SandboxBackend::Bwrap)
        );
        assert_eq!(
            SandboxBackend::parse("Firejail"),
            Some(SandboxBackend::Firejail)
        );
        // Policies, not backends.
        assert_eq!(SandboxBackend::parse("auto"), None);
        assert_eq!(SandboxBackend::parse("off"), None);
        assert!(SandboxBackend::is_off("OFF"));
        assert!(SandboxBackend::is_off("none"));
        assert!(SandboxBackend::is_auto("auto"));
        assert!(SandboxBackend::is_auto(""));
        assert!(!SandboxBackend::is_auto("bwrap"));
        assert!(!SandboxBackend::is_off("bwrap"));
        assert_eq!(SandboxBackend::Bwrap.to_string(), "bwrap");
        assert_eq!(SandboxBackend::Firejail.to_string(), "firejail");
    }

    /// The systemd-resolved layout: `/etc/resolv.conf` links out of `/etc`, so
    /// the target must be found and reported at its own absolute path.
    #[test]
    fn resolv_conf_target_follows_a_link_out_of_etc() {
        let tmp = tempfile::tempdir().unwrap();
        let etc = tmp.path().join("etc");
        let run = tmp.path().join("run/systemd/resolve");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        let real = run.join("stub-resolv.conf");
        std::fs::write(&real, "nameserver 127.0.0.53\n").unwrap();
        std::os::unix::fs::symlink(&real, etc.join("resolv.conf")).unwrap();

        assert_eq!(resolv_conf_target_at(&etc.join("resolv.conf")), Some(real));
    }

    /// A relative link (`-> ../run/...`) resolves against `/etc`, not the CWD.
    #[test]
    fn resolv_conf_target_resolves_relative_links_against_etc() {
        let tmp = tempfile::tempdir().unwrap();
        let etc = tmp.path().join("etc");
        let run = tmp.path().join("run/systemd/resolve");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("stub-resolv.conf"), "nameserver 1.1.1.1\n").unwrap();
        std::os::unix::fs::symlink(
            "../run/systemd/resolve/stub-resolv.conf",
            etc.join("resolv.conf"),
        )
        .unwrap();

        let got = resolv_conf_target_at(&etc.join("resolv.conf")).unwrap();
        assert_eq!(got, run.join("stub-resolv.conf").canonicalize().unwrap());
    }

    /// A regular file is already inside the `/etc` bind — binding it again would
    /// be pointless, and a link still under `/etc` likewise.
    #[test]
    fn resolv_conf_target_is_none_for_plain_files_and_internal_links() {
        let tmp = tempfile::tempdir().unwrap();
        let etc = tmp.path().join("etc");
        std::fs::create_dir_all(&etc).unwrap();

        // Regular file.
        let plain = etc.join("resolv.conf");
        std::fs::write(&plain, "nameserver 8.8.8.8\n").unwrap();
        assert_eq!(resolv_conf_target_at(&plain), None);

        // Symlink whose target is still under /etc.
        std::fs::write(etc.join("other.conf"), "nameserver 9.9.9.9\n").unwrap();
        let internal = etc.join("resolv-link.conf");
        std::os::unix::fs::symlink("other.conf", &internal).unwrap();
        assert_eq!(resolv_conf_target_at(&internal), None);

        // Dangling link: nothing to bind, and it must not panic.
        let dangling = etc.join("resolv-dangling.conf");
        std::os::unix::fs::symlink("/no/such/resolv.conf", &dangling).unwrap();
        assert_eq!(resolv_conf_target_at(&dangling), None);

        // Missing file entirely.
        assert_eq!(resolv_conf_target_at(&etc.join("nope.conf")), None);
    }

    /// Whatever the host's layout, the bind must target the resolved file at its
    /// own path — mounting onto the `/etc/resolv.conf` symlink itself is refused
    /// by bwrap ("Can't mount on symlink destination"), which is how the first
    /// attempt at this fix failed.
    #[test]
    fn bwrap_binds_dns_config_when_resolv_conf_is_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(tmp.path(), SandboxOptions::default());
        let (_, args) = sb.argv("true", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        match resolv_conf_target() {
            Some(target) => {
                let t = target.to_string_lossy().into_owned();
                assert!(
                    flat.windows(3)
                        .any(|w| w[0] == "--ro-bind-try" && w[1] == t && w[2] == t),
                    "DNS config {t} not bound: {flat:?}"
                );
                assert!(
                    !flat
                        .windows(3)
                        .any(|w| w[0] == "--ro-bind-try" && w[1] == "/etc/resolv.conf"),
                    "must not mount onto the /etc/resolv.conf symlink: {flat:?}"
                );
            }
            None => assert!(
                !flat.iter().any(|a| a == "--ro-bind-try"),
                "no extra DNS bind expected on this host: {flat:?}"
            ),
        }
    }

    /// firejail sees the host filesystem, so it never needs (and must not get)
    /// bwrap's mount flags.
    #[test]
    fn firejail_gets_no_dns_bind() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(
            SandboxBackend::Firejail,
            tmp.path(),
            SandboxOptions::default(),
        )
        .unwrap();
        assert!(
            sb.resolv_conf.is_none(),
            "firejail must not carry DNS state"
        );
        let (_, args) = sb.argv("true", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !flat.iter().any(|a| a == "--ro-bind-try"),
            "unexpected bwrap flag: {flat:?}"
        );
    }

    #[test]
    fn new_requires_existing_workspace_dir() {
        let err = Sandbox::new(
            SandboxBackend::Bwrap,
            Path::new("/definitely/not/a/real/dir"),
            SandboxOptions::default(),
        )
        .unwrap_err();
        assert!(
            err.contains("canonicalize") || err.contains("directory"),
            "{err}"
        );
    }

    #[test]
    fn workspace_is_canonicalized() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(tmp.path(), SandboxOptions::default());
        assert_eq!(sb.workspace(), tmp.path().canonicalize().unwrap());
        assert_eq!(sb.backend(), SandboxBackend::Bwrap);
    }

    #[test]
    fn bwrap_argv_contains_the_containment_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(tmp.path(), SandboxOptions::default());
        let (program, args) = sb.argv("echo hi", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert_eq!(program.file_name().unwrap(), "bwrap");
        assert_eq!(flat.last().map(|s| s.as_str()), Some("echo hi"));
        assert_eq!(flat[flat.len() - 3], "/bin/sh");
        assert_eq!(flat[flat.len() - 2], "-c");

        for flag in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
            "--tmpfs",
            "--proc",
            "--dev",
            "--chdir",
        ] {
            assert!(flat.iter().any(|a| a == flag), "missing {flag}: {flat:?}");
        }
        // The network must stay reachable: `git`/`curl`/`cargo add` are
        // everyday agent operations, so no net-isolation flag may appear.
        for flag in ["--unshare-net", "--net=none"] {
            assert!(
                !flat.iter().any(|a| a == flag),
                "unexpected {flag}: {flat:?}"
            );
        }
        // The host root must never be bound — that is what would make reads of
        // arbitrary paths possible.
        assert!(!flat.windows(2).any(|w| w[0] == "--ro-bind" && w[1] == "/"));
        assert!(!flat.windows(2).any(|w| w[0] == "--bind" && w[1] == "/"));
        // /usr and /etc are the minimum viable system view.
        assert!(
            flat.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == "/usr")
        );
        assert!(
            flat.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == "/etc")
        );
        // And the workspace is the last bind, read-write, so it wins.
        let ws = tmp
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let last_bind = flat
            .iter()
            .rposition(|a| a == "--bind")
            .expect("no rw bind");
        assert_eq!(flat[last_bind + 1], ws);
        assert_eq!(flat[last_bind + 2], ws);
    }

    #[test]
    fn bwrap_chdir_follows_explicit_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().canonicalize().unwrap().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let sb = sandbox_in(tmp.path(), SandboxOptions::default());
        let (_, args) = sb.argv("true", Some(&sub));
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let idx = flat.iter().rposition(|a| a == "--chdir").unwrap();
        assert_eq!(flat[idx + 1], sub.to_string_lossy());
    }

    #[test]
    fn bwrap_binds_extra_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let ro = tempfile::tempdir().unwrap();
        let rw = tempfile::tempdir().unwrap();
        let sb = sandbox_in(
            tmp.path(),
            SandboxOptions {
                read_only_paths: vec![ro.path().to_path_buf()],
                read_write_paths: vec![rw.path().to_path_buf()],
                ..SandboxOptions::default()
            },
        );
        let (_, args) = sb.argv("true", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let ro_s = ro
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let rw_s = rw
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(flat.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == ro_s));
        assert!(flat.windows(3).any(|w| w[0] == "--bind" && w[1] == rw_s));
    }

    #[test]
    fn missing_bind_targets_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let ghost = Path::new("/no/such/dir/anywhere");
        let sb = sandbox_in(
            tmp.path(),
            SandboxOptions {
                read_only_paths: vec![ghost.to_path_buf()],
                cargo_home: Some(ghost.to_path_buf()),
                ..SandboxOptions::default()
            },
        );
        let (_, args) = sb.argv("true", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !flat.iter().any(|a| a == "/no/such/dir/anywhere"),
            "{flat:?}"
        );
    }

    #[test]
    fn environment_is_a_whitelist() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let cargo_home = tempfile::tempdir().unwrap();
        let rustup = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let sb = sandbox_in(
            tmp.path(),
            SandboxOptions {
                home: Some(home.path().to_path_buf()),
                cargo_home: Some(cargo_home.path().to_path_buf()),
                rustup_home: Some(rustup.path().to_path_buf()),
                cargo_bin: Some(bin.path().to_path_buf()),
                ..SandboxOptions::default()
            },
        );
        let env = sb.environment();
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"PATH"));
        assert!(keys.contains(&"HOME"));
        assert!(keys.contains(&"CARGO_HOME"));
        assert!(keys.contains(&"RUSTUP_HOME"));
        // Nothing secret: the whole set is known up front (plus locale/TERM,
        // which come from the host and are harmless).
        for key in keys.iter() {
            assert!(
                matches!(
                    *key,
                    "PATH"
                        | "HOME"
                        | "CARGO_HOME"
                        | "RUSTUP_HOME"
                        | "LANG"
                        | "LC_ALL"
                        | "LC_CTYPE"
                        | "TERM"
                ),
                "unexpected env var leaked into sandbox: {key}"
            );
        }
        // cargo/rustc must be reachable through the prepended bin dir.
        let path = env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        assert!(
            path.starts_with(&bin.path().to_string_lossy().to_string()),
            "{path}"
        );
    }

    #[test]
    fn environment_omits_unbound_toolchain_paths() {
        // A $CARGO_HOME the sandbox cannot see would let cargo quietly create
        // an ephemeral one on the tmpfs root instead of failing visibly.
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(
            tmp.path(),
            SandboxOptions {
                cargo_home: Some(PathBuf::from("/no/such/cargo/home")),
                cargo_bin: Some(PathBuf::from("/no/such/bin")),
                ..SandboxOptions::default()
            },
        );
        let env = sb.environment();
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"CARGO_HOME"), "{keys:?}");
        assert!(!keys.contains(&"RUSTUP_HOME"), "{keys:?}");
        let path = env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        assert!(!path.contains("/no/such/bin"), "{path}");
    }

    #[test]
    fn home_is_never_bound_but_nested_paths_are() {
        // Binding $HOME would expose ~/.ssh, ~/.bash_history and friends to
        // sandboxed commands. The profile relies on bwrap creating mountpoint
        // parents on demand instead, so a nested bind works without it.
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let nested = home.path().join(".local/share/data");
        std::fs::create_dir_all(&nested).unwrap();

        let sb = sandbox_in(
            tmp.path(),
            SandboxOptions {
                home: Some(home.path().to_path_buf()),
                read_only_paths: vec![nested.clone()],
                ..SandboxOptions::default()
            },
        );
        let (_, args) = sb.argv("true", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        let home_s = home
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            !flat.windows(3).any(|w| w[1] == home_s && w[2] == home_s),
            "$HOME itself must never be bound: {flat:?}"
        );

        // The nested path is bound, and only it.
        let nested_s = nested.to_string_lossy().into_owned();
        assert!(
            flat.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == nested_s),
            "nested allowed path was not bound: {flat:?}"
        );
    }

    #[test]
    fn extra_env_reaches_the_sandboxed_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(tmp.path(), SandboxOptions::default());
        let extra = [("TH_QUERY".to_string(), "hello world".to_string())];
        let (_, args) = sb.argv_with_env("echo $TH_QUERY", None, &extra);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let idx = flat
            .iter()
            .position(|a| a == "TH_QUERY")
            .expect("TH_QUERY not passed through");
        assert_eq!(flat[idx - 1], "--setenv");
        assert_eq!(flat[idx + 1], "hello world");
        // The command itself is still the last three arguments.
        assert_eq!(flat[flat.len() - 3], "/bin/sh");
        assert_eq!(flat[flat.len() - 2], "-c");
        assert_eq!(flat[flat.len() - 1], "echo $TH_QUERY");
    }

    #[test]
    fn firejail_argv_shapes_expected_options() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(
            SandboxBackend::Firejail,
            tmp.path(),
            SandboxOptions::default(),
        )
        .unwrap();
        let (program, args) = sb.argv("ls", None);
        let flat: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(program.file_name().unwrap(), "firejail");
        let ws = tmp
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(flat.contains(&format!("--bind={ws}")), "{flat:?}");
        for flag in [
            "--noprofile",
            "--read-only",
            "--caps.drop=all",
            "--seccomp",
            "--nogroups",
        ] {
            assert!(flat.iter().any(|a| a == flag), "missing {flag}");
        }
        assert_eq!(flat.last().map(|s| s.as_str()), Some("ls"));
        // firejail never gets bwrap's mount flags.
        assert!(!flat.iter().any(|a| a == "--ro-bind"));
        // …and never isolates the network, same policy as bwrap.
        assert!(
            !flat.iter().any(|a| a == "--net=none"),
            "firejail must not block the network: {flat:?}"
        );
    }

    #[test]
    fn available_backends_are_locatable() {
        let avail = Sandbox::available();
        for backend in &avail {
            assert!(Sandbox::locate(*backend).is_some());
        }
        // Preference order is stable: bwrap before firejail.
        if avail.len() == ALL_BACKENDS.len() {
            assert_eq!(avail, ALL_BACKENDS.to_vec());
        }
    }

    #[test]
    fn detect_either_succeeds_or_explains_itself() {
        let tmp = tempfile::tempdir().unwrap();
        match Sandbox::detect(tmp.path(), SandboxOptions::default()) {
            Ok(sb) => assert!(
                ALL_BACKENDS.contains(&sb.backend()),
                "unexpected backend {:?}",
                sb.backend()
            ),
            Err(e) => assert!(
                e.contains("bubblewrap") && e.contains("firejail"),
                "unhelpful error: {e}"
            ),
        }
    }

    #[test]
    fn from_setting_honours_explicit_backend_and_rejects_off() {
        let tmp = tempfile::tempdir().unwrap();
        // `off` is a policy the caller must handle before building.
        assert!(
            Sandbox::from_setting("off", tmp.path(), SandboxOptions::default())
                .unwrap_err()
                .contains("disables sandboxing")
        );
        // Unknown names are rejected with the valid list.
        let err =
            Sandbox::from_setting("docker", tmp.path(), SandboxOptions::default()).unwrap_err();
        assert!(err.contains("Unknown sandbox backend"), "{err}");
        assert!(err.contains("bwrap"), "{err}");
        // An installed backend builds; a missing one says so.
        match Sandbox::from_setting("bwrap", tmp.path(), SandboxOptions::default()) {
            Ok(sb) => assert_eq!(sb.backend(), SandboxBackend::Bwrap),
            Err(e) => assert!(e.contains("not installed"), "{e}"),
        }
        // `auto` behaves like detect(): ok iff some backend is installed.
        let auto = Sandbox::from_setting("auto", tmp.path(), SandboxOptions::default());
        assert_eq!(auto.is_ok(), !Sandbox::available().is_empty());
    }

    #[test]
    fn symlink_detection_ignores_real_dirs_and_missing_paths() {
        // Merged-usr probing must not panic on a plain directory or a path
        // that does not exist.
        assert!(!is_symlink_to(Path::new("/definitely/missing"), "usr/bin"));
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_symlink_to(tmp.path(), "usr/bin"));

        // The links this actually looks for are *relative* (`/bin -> usr/bin`),
        // so an absolute-target symlink must not be mistaken for one.
        let rel = tmp.path().join("rel");
        std::os::unix::fs::symlink("usr/bin", &rel).unwrap();
        assert!(is_symlink_to(&rel, "usr/bin"));
        assert!(!is_symlink_to(&rel, "usr/lib"));

        let abs = tmp.path().join("abs");
        std::os::unix::fs::symlink("/usr/bin", &abs).unwrap();
        assert!(!is_symlink_to(&abs, "usr/bin"));
    }
}
