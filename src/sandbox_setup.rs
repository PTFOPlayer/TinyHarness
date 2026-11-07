//! Resolve the CLI sandbox flags and settings into a concrete sandbox and
//! path guard for this session.
//!
//! Two independent containment layers are decided here:
//!
//! 1. **Path guard** — confines the built-in file tools (`ls`, `read`,
//!    `write`, `edit`, `grep`, `glob`) to the workspace plus `allowed_paths`.
//!    Always on; there is no way to turn it off.
//! 2. **Process sandbox** — wraps `run` and custom tools in bwrap/firejail.
//!    **On by default** (`sandbox.backend` defaults to `auto`); opt out with
//!    `--no-sandbox`, or pin a backend with `--sandbox=bwrap|firejail`.
//!
//! Precedence for the process sandbox: an explicit `--sandbox…` flag wins over
//! settings, and `--no-sandbox` (like `--sandbox=off`) forces it off for one
//! session. A bare `--sandbox` means "auto". The setting is global-only — a
//! project `.tinyharness/config.json` cannot reach it, so a cloned repository
//! cannot downgrade the user's containment. See [`SandboxConfig`].
//!
//! Failure handling: because sandboxing is the default rather than an opt-in,
//! a sandbox that was *expected* but cannot be built is a **hard exit** — the
//! alternative is silently running every shell command unsandboxed, which is
//! exactly the failure the flag exists to prevent. Only an explicit
//! `--no-sandbox` opts out of that, and it opts out loudly.

use std::path::{Path, PathBuf};

use tinyharness_lib::{
    MergedSettings, Paths,
    config::SandboxConfig,
    path_guard::PathGuard,
    sandbox::{Sandbox, SandboxOptions},
};

use crate::commands::registry::SandboxStatus;

/// What `--sandbox` said, after clap has parsed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliSandbox {
    /// Flag absent — settings decide.
    Unspecified,
    /// Bare `--sandbox` — auto-detect.
    Auto,
    /// `--sandbox=<value>` — an explicit backend name, or `off`.
    Named(String),
}

impl CliSandbox {
    /// Interpret clap's `Option<Option<String>>`.
    pub fn from_arg(value: &Option<Option<String>>) -> Self {
        match value {
            None => CliSandbox::Unspecified,
            Some(None) => CliSandbox::Auto,
            Some(Some(v)) => CliSandbox::Named(v.clone()),
        }
    }
}

/// The resolved containment configuration for a session.
#[derive(Debug)]
pub struct Resolution {
    /// The path guard confining the built-in file tools. Always present —
    /// this layer has no off switch.
    pub path_guard: PathGuard,
    /// The process sandbox, if one is active.
    pub sandbox: Option<Sandbox>,
    /// Status summary for the system prompt (`None` when unsandboxed).
    pub status: Option<SandboxStatus>,
    /// Non-fatal problems worth telling the user about (a settings-driven
    /// sandbox that could not be built, an unusable `allowed_paths` entry).
    pub warnings: Vec<String>,
}

/// The effective sandbox configuration after applying CLI precedence.
///
/// Split out from [`resolve`] so the precedence rules are testable without
/// touching the filesystem or spawning a probe.
pub fn effective_config(cli: &CliSandbox, settings: &SandboxConfig) -> SandboxConfig {
    match cli {
        CliSandbox::Unspecified => settings.clone(),
        CliSandbox::Auto => SandboxConfig {
            backend: "auto".to_string(),
            ..settings.clone()
        },
        CliSandbox::Named(value) => SandboxConfig {
            backend: if value.trim().is_empty() {
                // `--sandbox=` — treat as auto rather than as an invalid name.
                "auto".to_string()
            } else {
                value.clone()
            },
            ..settings.clone()
        },
    }
}

/// Build the [`SandboxOptions`] for a session from settings and the host
/// toolchain layout.
///
/// `writable_cargo_home` is the directory exposed as `$CARGO_HOME` inside the
/// sandbox. The host's `~/.cargo` is bound **read-only** — so cargo's
/// package-cache lock cannot be taken there — and this substitute keeps
/// `cargo`/`rustc` invocations working without writing into the user's real
/// cargo directory. The trade-off is an empty registry: a sandboxed build that
/// needs to *download* crates will fail, while one that only needs the
/// already-installed toolchain and vendored deps works.
fn sandbox_options(
    config: &SandboxConfig,
    allowed: &[String],
    paths: &Paths,
    writable_cargo_home: &Path,
) -> SandboxOptions {
    let home = paths.home().map(|p| p.to_path_buf());

    // The host toolchain, located the way cargo itself would.
    let host_cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    let cargo_bin = host_cargo_home
        .as_ref()
        .map(|h| h.join("bin"))
        .filter(|p| p.is_dir());
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".rustup")))
        .filter(|p| p.is_dir());

    // `allowed_paths` feeds both layers: the path guard (in-process) and the
    // sandbox's read-only binds, so `read` and `run` see the same extra trees.
    let read_only = resolve_dirs(allowed, &home, paths.project_dir());
    let read_write = resolve_dirs(&config.writable_paths, &home, paths.project_dir());

    SandboxOptions {
        read_only_paths: read_only,
        read_write_paths: read_write,
        home,
        cargo_home: Some(writable_cargo_home.to_path_buf()),
        rustup_home,
        cargo_bin,
    }
}

/// Turn raw config entries into existing absolute directories, skipping (with
/// a warning) anything that is missing — a stale path in settings should not
/// prevent the rest of the sandbox from working.
fn resolve_dirs(entries: &[String], home: &Option<PathBuf>, project: &Path) -> Vec<PathBuf> {
    entries
        .iter()
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return None;
            }
            let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
                home.as_ref()?.join(rest)
            } else if trimmed == "~" {
                home.clone()?
            } else if Path::new(trimmed).is_absolute() {
                PathBuf::from(trimmed)
            } else {
                project.join(trimmed)
            };
            if expanded.is_dir() {
                Some(expanded)
            } else {
                tracing::warn!(
                    "sandbox path '{trimmed}' resolves to '{}' which is not a directory — skipping",
                    expanded.display()
                );
                None
            }
        })
        .collect()
}

/// Resolve the session's containment layers.
///
/// `required` selects the failure mode for a sandbox that cannot be built:
/// `true` (the default — the user never opted out) returns `Err` so the caller
/// exits, because continuing would mean unsandboxed shell commands under a user
/// who believes they are sandboxed. `false` (`--no-sandbox`, or
/// `sandbox.backend = "off"`) skips the sandbox entirely and never reaches
/// this path.
pub fn resolve(
    cli: &CliSandbox,
    merged: &MergedSettings,
    paths: &Paths,
    required: bool,
) -> Result<Resolution, String> {
    let config = effective_config(cli, &merged.sandbox);
    let mut warnings = Vec::new();

    let effective_allowed = {
        let mut entries = merged.allowed_paths.clone();
        // Deduplicate: project entries extend global, and the same path in
        // both would otherwise appear twice in the banner and the binds.
        entries.sort();
        entries.dedup();
        entries
    };

    let guard = build_guard(paths, &effective_allowed, &mut warnings)?;

    if config.is_disabled() {
        return Ok(Resolution {
            path_guard: guard,
            sandbox: None,
            status: None,
            warnings,
        });
    }

    let writable_cargo_home = paths.data_dir().join("sandbox").join("cargo-home");
    if let Err(e) = std::fs::create_dir_all(&writable_cargo_home) {
        warnings.push(format!(
            "could not create the sandbox cargo home '{}': {e} — sandboxed \
             cargo commands will fail to lock the registry",
            writable_cargo_home.display()
        ));
    }

    let options = sandbox_options(&config, &effective_allowed, paths, &writable_cargo_home);

    let sandbox = match Sandbox::from_setting(&config.backend, paths.project_dir(), options) {
        Ok(sandbox) => sandbox,
        Err(e) if required => return Err(e),
        Err(e) => {
            warnings.push(format!(
                "settings requested sandbox backend '{}' but it is unavailable: {e}",
                config.backend
            ));
            return Ok(Resolution {
                path_guard: guard,
                sandbox: None,
                status: None,
                warnings,
            });
        }
    };

    // Installed but unusable (firejail without its setuid bit, a distro with
    // unprivileged user namespaces disabled) — catch it here rather than on
    // the first tool call.
    if let Err(cause) = sandbox.probe() {
        let msg = format!(
            "sandbox backend '{}' is installed but could not run a test command: {cause}",
            sandbox.backend()
        );
        if required {
            return Err(msg);
        }
        warnings.push(msg);
        return Ok(Resolution {
            path_guard: guard,
            sandbox: None,
            status: None,
            warnings,
        });
    }

    let status = SandboxStatus {
        backend: sandbox.backend().to_string(),
        extra_paths: sandbox.options().read_only_paths.clone(),
    };
    Ok(Resolution {
        path_guard: guard,
        sandbox: Some(sandbox),
        status: Some(status),
        warnings,
    })
}

/// Build the path guard, tolerating a bad `allowed_paths` entry.
///
/// A guard that cannot honour its configuration is still far better than no
/// guard at all, so an unusable `allowed_paths` setting degrades to the
/// workspace-only root with a warning instead of failing the session.
fn build_guard(
    paths: &Paths,
    allowed: &[String],
    warnings: &mut Vec<String>,
) -> Result<PathGuard, String> {
    match PathGuard::builder(paths.project_dir())
        .allowed_paths(allowed.iter().cloned())
        .home(paths.home())
        .build()
    {
        Ok(guard) => Ok(guard),
        Err(e) => {
            warnings.push(format!(
                "ignoring allowed_paths ({e}) — file tools stay confined to the workspace root"
            ));
            PathGuard::builder(paths.project_dir())
                .home(paths.home())
                .build()
        }
    }
}

/// Describe what the process sandbox will do, for the startup banner.
pub fn describe(sandbox: &Sandbox, guard_root: &Path) -> Vec<String> {
    let mut lines = vec![format!(
        "shell commands are confined by {} to {}",
        sandbox.backend(),
        sandbox.workspace().display()
    )];
    // Stated unconditionally: it is a limit of the sandbox, not a setting, and
    // a security banner that stays silent about it invites false confidence.
    lines.push("network: available (not isolated by the sandbox)".to_string());
    lines.push(format!(
        "file tools are confined to {}",
        guard_root.display()
    ));
    let extras = &sandbox.options().read_only_paths;
    if !extras.is_empty() {
        lines.push(format!(
            "extra read-only paths: {}",
            extras
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let writable = &sandbox.options().read_write_paths;
    if !writable.is_empty() {
        lines.push(format!(
            "extra writable paths (sandboxed commands only): {}",
            writable
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
}

/// A settings sandbox block for the tests below.
///
/// Lives at module scope (rather than inside a test module) because two test
/// modules use it.
#[cfg(test)]
fn test_sandbox_config(backend: &str) -> SandboxConfig {
    SandboxConfig {
        backend: backend.to_string(),
        writable_paths: vec!["/tmp/x".to_string()],
    }
}

// Pure precedence/parsing logic — meaningful and portable on every platform,
// so these are deliberately *not* unix-gated (CI builds Windows and macOS).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_absent_defers_to_settings() {
        let settings = test_sandbox_config("firejail");
        let eff = effective_config(&CliSandbox::Unspecified, &settings);
        assert_eq!(eff.backend, "firejail");
        // Untouched: writable paths carry through.
        assert_eq!(eff.writable_paths, vec!["/tmp/x".to_string()]);
    }

    #[test]
    fn bare_flag_forces_auto_but_keeps_other_settings() {
        let settings = test_sandbox_config("off");
        let eff = effective_config(&CliSandbox::Auto, &settings);
        assert_eq!(eff.backend, "auto");
        // The flag selects the backend, not the whole policy.
        assert_eq!(eff.writable_paths, vec!["/tmp/x".to_string()]);
    }

    #[test]
    fn named_flag_overrides_settings_backend() {
        let settings = test_sandbox_config("auto");
        let eff = effective_config(&CliSandbox::Named("bwrap".into()), &settings);
        assert_eq!(eff.backend, "bwrap");
    }

    #[test]
    fn flag_off_forces_disable_even_when_settings_enable() {
        let settings = test_sandbox_config("bwrap");
        let eff = effective_config(&CliSandbox::Named("off".into()), &settings);
        assert!(eff.is_disabled());
    }

    #[test]
    fn empty_named_value_is_auto_not_invalid() {
        let settings = test_sandbox_config("off");
        let eff = effective_config(&CliSandbox::Named("  ".into()), &settings);
        assert_eq!(eff.backend, "auto");
        assert!(eff.is_enabled());
    }

    #[test]
    fn cli_parsing_shapes() {
        assert_eq!(CliSandbox::from_arg(&None), CliSandbox::Unspecified);
        assert_eq!(CliSandbox::from_arg(&Some(None)), CliSandbox::Auto);
        assert_eq!(
            CliSandbox::from_arg(&Some(Some("bwrap".into()))),
            CliSandbox::Named("bwrap".into())
        );
    }

    #[test]
    fn config_predicates_classify_every_spelling() {
        // The backend string decides whether a sandbox is requested at all, so
        // every accepted spelling must land in exactly one bucket.
        for value in ["off", "none", "no", "false", "0", "disabled"] {
            assert!(
                SandboxConfig {
                    backend: value.into(),
                    ..SandboxConfig::disabled()
                }
                .is_disabled(),
                "'{value}' should mean disabled"
            );
        }
        for value in ["auto", "bwrap", "firejail", "bubblewrap"] {
            assert!(
                SandboxConfig {
                    backend: value.into(),
                    ..SandboxConfig::disabled()
                }
                .is_enabled(),
                "'{value}' should mean enabled"
            );
        }
    }
}

// Tests that touch the filesystem or construct a sandbox. `canonicalize()`
// behaves differently on Windows (verbatim `\\?\` prefixes), so these follow
// the codebase convention of running on unix only; the precedence logic above
// is platform-independent and runs everywhere.
#[cfg(all(test, unix))]
mod fs_tests {
    use super::*;
    use tinyharness_lib::config::SettingSource;
    use tinyharness_lib::sandbox::SandboxBackend;

    #[test]
    fn resolve_dirs_skips_missing_and_expands_tilde() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let existing = home.path().join(".local/share/data");
        std::fs::create_dir_all(&existing).unwrap();
        let home_buf = Some(home.path().to_path_buf());

        let entries = vec![
            "~/.local/share/data".to_string(),
            "/no/such/path/anywhere".to_string(),
            "relative-dir".to_string(),
            "   ".to_string(),
        ];
        std::fs::create_dir_all(tmp.path().join("relative-dir")).unwrap();

        let resolved = resolve_dirs(&entries, &home_buf, tmp.path());
        assert_eq!(resolved.len(), 2, "{resolved:?}");
        assert!(resolved.contains(&existing));
        assert!(resolved.contains(&tmp.path().join("relative-dir")));
    }

    #[test]
    fn resolve_dirs_without_home_skips_tilde_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vec!["~/.cargo".to_string()];
        assert!(resolve_dirs(&entries, &None, tmp.path()).is_empty());
    }

    /// Sandboxing is the default, so a session with no CLI flag and no
    /// `sandbox` setting must *require* a sandbox: it either gets one or the
    /// startup fails loudly. Silently continuing unsandboxed is the exact
    /// regression this test guards against.
    #[test]
    fn default_session_requires_a_sandbox_or_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_home(tmp.path()).with_project_dir(tmp.path());
        let (_global, _project, merged) =
            tinyharness_lib::config::load_merged_settings_from(&paths);
        assert!(
            merged.sandbox.is_enabled(),
            "the default settings must ask for a sandbox, got {:?}",
            merged.sandbox.backend
        );

        match resolve(&CliSandbox::Unspecified, &merged, &paths, true) {
            // A backend is installed: it must be wired up, not skipped.
            Ok(res) => {
                assert!(
                    res.sandbox.is_some(),
                    "default session came out unsandboxed"
                );
                assert!(res.status.is_some());
            }
            // No backend: a hard error is correct — the caller exits.
            Err(e) => assert!(
                Sandbox::available().is_empty(),
                "resolve failed although a backend is installed: {e}"
            ),
        }
    }

    /// `--no-sandbox` is the only opt-out, and it still gets the path guard:
    /// the file-tool layer has no off switch.
    #[test]
    fn explicit_opt_out_yields_no_sandbox_but_still_a_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_home(tmp.path()).with_project_dir(tmp.path());
        let (_g, _p, merged) = tinyharness_lib::config::load_merged_settings_from(&paths);
        let res = resolve(&CliSandbox::Named("off".into()), &merged, &paths, false).unwrap();
        assert!(res.sandbox.is_none());
        assert!(res.status.is_none());
        assert!(
            res.warnings.is_empty(),
            "an explicit opt-out should not warn: {:?}",
            res.warnings
        );
        // Guard is always produced — that is the point of the split.
        assert_eq!(
            res.path_guard.root(),
            tmp.path().canonicalize().unwrap(),
            "the path guard must survive an opt-out"
        );
    }

    /// A project `.tinyharness/config.json` cannot downgrade the sandbox: it
    /// has no `sandbox` field at all, so a cloned repository cannot turn off
    /// containment the user configured globally.
    #[test]
    fn project_config_cannot_override_the_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".config/tinyharness")).unwrap();
        std::fs::write(
            home.path().join(".config/tinyharness/settings.json"),
            r#"{"sandbox": {"backend": "bwrap"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join(".tinyharness")).unwrap();
        // A hostile project tries both an explicit disable and a weaker backend.
        for attack in [
            r#"{"sandbox": {"backend": "off"}}"#,
            r#"{"sandbox": {"backend": "firejail"}}"#,
            r#"{"sandbox": "off"}"#,
        ] {
            std::fs::write(project.path().join(".tinyharness/config.json"), attack).unwrap();
            let paths = Paths::with_home(home.path()).with_project_dir(project.path());
            let (_g, _p, merged) = tinyharness_lib::config::load_merged_settings_from(&paths);
            assert_eq!(
                merged.sandbox.backend, "bwrap",
                "project config {attack} changed the sandbox"
            );
            assert_eq!(merged.sandbox_source, SettingSource::Global);
        }
    }

    #[test]
    fn resolve_reports_unusable_backend_loudly_when_required() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_home(tmp.path()).with_project_dir(tmp.path());
        let (_g, _p, merged) = tinyharness_lib::config::load_merged_settings_from(&paths);
        // An explicit but bogus backend is a hard error when required.
        let err = resolve(&CliSandbox::Named("docker".into()), &merged, &paths, true).unwrap_err();
        assert!(err.contains("Unknown sandbox backend"), "{err}");
    }

    #[test]
    fn resolve_downgrades_to_warning_when_not_required() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_home(tmp.path()).with_project_dir(tmp.path());
        let (_g, _p, merged) = tinyharness_lib::config::load_merged_settings_from(&paths);
        let res = resolve(
            &CliSandbox::Named("definitely-not-a-backend".into()),
            &merged,
            &paths,
            false,
        )
        .unwrap();
        assert!(res.sandbox.is_none());
        assert!(
            res.warnings
                .iter()
                .any(|w| w.contains("definitely-not-a-backend")),
            "warnings: {:?}",
            res.warnings
        );
    }

    #[test]
    fn describe_lists_the_policy_in_force() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(
            SandboxBackend::Bwrap,
            tmp.path(),
            SandboxOptions {
                read_only_paths: vec![extra.path().to_path_buf()],
                ..SandboxOptions::default()
            },
        )
        .unwrap();
        let lines = describe(&sandbox, tmp.path());
        let joined = lines.join("\n");
        assert!(joined.contains("bwrap"), "{joined}");
        // The banner must state the network limitation, not imply isolation.
        assert!(joined.contains("not isolated"), "{joined}");
        assert!(
            joined.contains(&extra.path().to_string_lossy().to_string()),
            "{joined}"
        );
    }
}
