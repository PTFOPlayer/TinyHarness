//! Central resolution of every filesystem location TinyHarness touches.
//!
//! Previously each module hard-coded its own `~/.config/tinyharness/...` or
//! `~/.local/share/tinyharness/...` path by reading `$HOME` inline, which made
//! the library impossible to drive against an alternate root (tests, embedded
//! use, portable installs, a second profile). Every such location now lives in
//! [`Paths`], and every subsystem accepts a `&Paths` (or an explicit `Path`).
//!
//! # Resolution rules
//!
//! | Logical location | Default |
//! |---|---|
//! | config dir | `$XDG_CONFIG_HOME/tinyharness` → `$HOME/.config/tinyharness` → `.config/tinyharness` |
//! | data dir | `$XDG_DATA_HOME/tinyharness` → `$HOME/.local/share/tinyharness` → `.local/share/tinyharness` |
//! | project dir | process CWD → `.` |
//!
//! The relative fallbacks preserve the pre-existing behaviour when `HOME` is
//! unset (the old code did `home.unwrap_or(".")`, yielding `./.config/...`), so
//! a caller with no home directory still gets a writable, self-consistent tree
//! instead of a panic.
//!
//! Per the XDG spec, `XDG_*_HOME` values that are empty or relative are
//! ignored (a relative path is not an absolute path, so it cannot be resolved
//! predictably against a project root); the `$HOME` fallback is used instead.
//!
//! # Legacy locations
//!
//! Personal skills historically lived in `~/.tinyharness/skills`. That
//! directory is still searched, *after* `config_dir/skills`, so an existing
//! install keeps working while the documented XDG location takes precedence.
//! See [`Paths::personal_skills_dirs`].
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//! use tinyharness_lib::paths::Paths;
//!
//! // Drive the library against a sandbox root instead of the real home dir.
//! let paths = Paths::with_home(PathBuf::from("/tmp/th-profile"));
//! let store = tinyharness_lib::session::SessionStore::from_paths(&paths);
//! ```

use std::path::{Path, PathBuf};

/// Directory name appended to the XDG config/data roots.
const APP_DIR: &str = "tinyharness";

/// Legacy personal dot-directory name (`~/.tinyharness`).
const LEGACY_DOT_DIR: &str = ".tinyharness";

/// Fallback config root when neither `XDG_CONFIG_HOME` nor `HOME` is usable.
const FALLBACK_CONFIG_ROOT: &str = ".config";

/// Fallback data root when neither `XDG_DATA_HOME` nor `HOME` is usable.
const FALLBACK_DATA_ROOT: &str = ".local/share";

/// Every filesystem location TinyHarness needs, fully injectable.
///
/// Construct with [`Paths::default`] (real environment), [`Paths::with_home`]
/// (a single root, ideal for tests and portable installs), [`Paths::from_env`]
/// (an injected environment map, for hermetic tests), or [`Paths::new`] plus
/// the `with_*` builders for arbitrary layouts.
///
/// All getters derive their paths on demand, so overriding one root
/// automatically re-derives every file beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: Option<PathBuf>,
    config_dir: PathBuf,
    data_dir: PathBuf,
    project_dir: PathBuf,
}

impl Default for Paths {
    /// Resolve from the real process environment and CWD.
    fn default() -> Self {
        Self::from_env(|key| std::env::var(key).ok())
    }
}

impl Paths {
    /// Build from an injected environment lookup.
    ///
    /// `env` returns `None` for unset variables. This is the seam for hermetic
    /// tests: no process-global `set_var` (and its parallel-test races) needed.
    ///
    /// `HOME`, `USERPROFILE`, and the `XDG_*` roots must be absolute, following
    /// the XDG rule that a relative root is ignored. Absoluteness is
    /// platform-dependent — `Path::is_absolute` treats `/home/u` as *relative*
    /// on Windows — so a POSIX-style value inherited from an MSYS/Git Bash
    /// parent shell is rejected there. Note that the lookup is `HOME` then
    /// `USERPROFILE`, and the absolute check happens *after*: a `HOME` that is
    /// set but unusable leaves `home()` as `None` rather than falling through
    /// to `USERPROFILE`, and the config/data roots degrade to the `.`-relative
    /// fallback. `Paths` itself never fails, so the tree stays self-consistent.
    ///
    /// The project directory always comes from the process CWD — it is a
    /// working location, not a configuration value — falling back to `.` if it
    /// cannot be determined.
    pub fn from_env<F>(env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let home = env("HOME")
            .or_else(|| env("USERPROFILE"))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());

        let config_dir = xdg_dir(&env, "XDG_CONFIG_HOME")
            .map(|root| root.join(APP_DIR))
            .unwrap_or_else(|| {
                home.as_ref()
                    .map(|h| h.join(FALLBACK_CONFIG_ROOT))
                    .unwrap_or_else(|| PathBuf::from(FALLBACK_CONFIG_ROOT))
                    .join(APP_DIR)
            });

        let data_dir = xdg_dir(&env, "XDG_DATA_HOME")
            .map(|root| root.join(APP_DIR))
            .unwrap_or_else(|| {
                home.as_ref()
                    .map(|h| h.join(FALLBACK_DATA_ROOT))
                    .unwrap_or_else(|| PathBuf::from(FALLBACK_DATA_ROOT))
                    .join(APP_DIR)
            });

        let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Paths {
            home,
            config_dir,
            data_dir,
            project_dir,
        }
    }

    /// Derive every location from a single home directory.
    ///
    /// Ignores `XDG_*` entirely: config goes to `<home>/.config/tinyharness`
    /// and data to `<home>/.local/share/tinyharness`. This is the simplest way
    /// to point the library at a disposable root.
    ///
    /// The project directory is set to `home` itself, so the resulting layout
    /// is fully self-contained: nothing outside `home` is read. Note the
    /// consequence — `<home>/.tinyharness/` then acts as *both* the legacy
    /// personal-skills root and the project `.tinyharness/` directory. Use
    /// [`Paths::new`] + [`Paths::with_legacy_home`] to keep those distinct, as
    /// a real install does.
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Paths {
            config_dir: home.join(FALLBACK_CONFIG_ROOT).join(APP_DIR),
            data_dir: home.join(FALLBACK_DATA_ROOT).join(APP_DIR),
            project_dir: home.clone(),
            home: Some(home),
        }
    }

    /// Build from explicit roots, with no implicit home.
    ///
    /// Because there is no `home`, the legacy `~/.tinyharness/skills` directory
    /// is not searched — only `<config_dir>/skills`. Use
    /// [`Paths::with_legacy_home`] if you also need the legacy location.
    pub fn new(config_dir: PathBuf, data_dir: PathBuf, project_dir: PathBuf) -> Self {
        Paths {
            home: None,
            config_dir,
            data_dir,
            project_dir,
        }
    }

    /// Override the config directory (default: `<home>/.config/tinyharness`).
    pub fn with_config_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.config_dir = dir.into();
        self
    }

    /// Override the data directory (default: `<home>/.local/share/tinyharness`).
    pub fn with_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = dir.into();
        self
    }

    /// Override the project directory that `.tinyharness/` discovery starts from.
    pub fn with_project_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.project_dir = dir.into();
        self
    }

    /// Record a home directory so the legacy `~/.tinyharness/skills` location
    /// is searched, without changing any other root.
    pub fn with_legacy_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    // ── Roots ───────────────────────────────────────────────────────────

    /// The app's config directory (e.g. `~/.config/tinyharness`).
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// The app's data directory (e.g. `~/.local/share/tinyharness`).
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Directory that `.tinyharness/` project discovery walks up from.
    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// Resolved home directory, if one was available.
    ///
    /// `None` for layouts built via [`Paths::new`] with no home, or when the
    /// environment exposed no usable `HOME`.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    // ── Global (per-user) files ──────────────────────────────────────────

    /// `~/.config/tinyharness/settings.json`
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    /// `~/.config/tinyharness/prompts/`
    pub fn prompts_dir(&self) -> PathBuf {
        self.config_dir.join("prompts")
    }

    /// `~/.config/tinyharness/custom_tools.json`
    pub fn custom_tools_file(&self) -> PathBuf {
        self.config_dir.join("custom_tools.json")
    }

    /// `~/.local/share/tinyharness/sessions/`
    pub fn sessions_dir(&self) -> PathBuf {
        self.data_dir.join("sessions")
    }

    /// Directory holding the readline history file.
    ///
    /// Same as [`Paths::data_dir`]; exposed separately so callers express
    /// intent instead of reaching into the data root.
    pub fn history_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    /// `~/.local/share/tinyharness/audit.jsonl`
    pub fn audit_log_file(&self) -> PathBuf {
        self.data_dir.join("audit.jsonl")
    }

    /// `~/.local/share/tinyharness/backups/`
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Personal skill directories, in increasing precedence order.
    ///
    /// `<config_dir>/skills` first (the documented location), then the legacy
    /// `~/.tinyharness/skills` when a home directory is known. Duplicates are
    /// collapsed, so a layout where both resolve to the same path yields a
    /// single entry. Earlier entries win on name collisions — see
    /// [`crate::skill::discover_skills_in`].
    pub fn personal_skills_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.config_dir.join("skills")];
        if let Some(home) = self.home.as_deref() {
            let legacy = home.join(LEGACY_DOT_DIR).join("skills");
            if !dirs.contains(&legacy) {
                dirs.push(legacy);
            }
        }
        dirs
    }

    // ── Project-scoped files ─────────────────────────────────────────────

    /// `<project_dir>/.tinyharness/`
    pub fn project_config_root(&self) -> PathBuf {
        self.project_dir.join(LEGACY_DOT_DIR)
    }

    /// Starting point for the `.tinyharness/config.json` walk-up.
    ///
    /// Discovery itself walks ancestors, so the returned path is only the
    /// seed — see [`crate::config::discover_project_settings`].
    pub fn project_settings_file(&self) -> PathBuf {
        self.project_config_root().join("config.json")
    }

    /// Starting point for the `.tinyharness/custom_tools.json` walk-up.
    pub fn project_custom_tools_file(&self) -> PathBuf {
        self.project_config_root().join("custom_tools.json")
    }

    /// `<project_dir>/.tinyharness/skills/`
    pub fn project_skills_dir(&self) -> PathBuf {
        self.project_config_root().join("skills")
    }
}

/// Read an XDG root, honouring the spec's "unset or empty means default" and
/// "must be absolute" rules.
fn xdg_dir<F>(env: &F, key: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    env(key)
        .map(|value| (value.trim().to_string(), PathBuf::from(&value)))
        .filter(|(raw, path)| !raw.is_empty() && path.is_absolute())
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an immutable env map so tests never mutate process globals.
    /// The returned closure owns its map, so it is `'static`.
    fn env_map(entries: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    fn abs(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    /// Build a root that is absolute on *this* platform.
    ///
    /// `Path::is_absolute` is platform-dependent: `/home/u` is relative on
    /// Windows, and `C:/u` is relative on Linux. `Paths::from_env` rejects
    /// relative `HOME`/`USERPROFILE`/`XDG_*` values (the XDG spec requires
    /// ignoring non-absolute roots), so a hard-coded POSIX fixture does not
    /// merely look wrong on Windows — it is silently discarded and every
    /// assertion collapses onto the `./.config` fallback. Deriving fixtures
    /// from the CWD keeps them absolute everywhere.
    ///
    /// Nothing is created on disk: `Paths` only computes strings.
    fn host_root(tag: &str) -> PathBuf {
        std::env::current_dir()
            .expect("CWD is absolute on every supported platform")
            .join(format!("th-{tag}"))
    }

    /// [`host_root`] as an environment-variable value.
    fn env_value(root: &Path) -> &str {
        root.to_str().expect("fixtures are valid UTF-8")
    }

    #[test]
    fn home_fallback_when_no_xdg() {
        let home = host_root("home");
        let paths = Paths::from_env(env_map(&[("HOME", env_value(&home))]));
        assert_eq!(paths.config_dir(), home.join(".config/tinyharness"));
        assert_eq!(paths.data_dir(), home.join(".local/share/tinyharness"));
        assert_eq!(paths.home(), Some(home.as_path()));
    }

    #[test]
    fn xdg_roots_take_precedence_over_home() {
        let home = host_root("home");
        let config = host_root("xdg-config");
        let data = host_root("xdg-data");
        let paths = Paths::from_env(env_map(&[
            ("HOME", env_value(&home)),
            ("XDG_CONFIG_HOME", env_value(&config)),
            ("XDG_DATA_HOME", env_value(&data)),
        ]));
        assert_eq!(paths.config_dir(), config.join("tinyharness"));
        assert_eq!(paths.data_dir(), data.join("tinyharness"));
    }

    #[test]
    fn empty_or_relative_xdg_is_ignored() {
        let home = host_root("home");
        let empty = Paths::from_env(env_map(&[
            ("HOME", env_value(&home)),
            ("XDG_CONFIG_HOME", "   "),
            // Relative on every supported platform, so this exercises the
            // "must be absolute" rule rather than platform quirks.
            ("XDG_DATA_HOME", "relative/data"),
        ]));
        assert_eq!(empty.config_dir(), home.join(".config/tinyharness"));
        assert_eq!(empty.data_dir(), home.join(".local/share/tinyharness"));
    }

    #[test]
    fn missing_home_uses_dot_prefixed_fallbacks() {
        // Preserves the historical `unwrap_or(".")` behaviour: the tree stays
        // self-consistent and writable rather than panicking.
        let paths = Paths::from_env(env_map(&[]));
        assert_eq!(paths.config_dir(), abs(".config/tinyharness"));
        assert_eq!(paths.data_dir(), abs(".local/share/tinyharness"));
        assert_eq!(paths.home(), None);
        assert_eq!(
            paths.settings_file(),
            abs(".config/tinyharness/settings.json")
        );
    }

    #[test]
    fn userprofile_is_home_fallback() {
        let home = host_root("userprofile-home");
        let paths = Paths::from_env(env_map(&[("USERPROFILE", env_value(&home))]));
        assert_eq!(paths.config_dir(), home.join(".config/tinyharness"));
        assert_eq!(paths.home(), Some(home.as_path()));
    }

    #[test]
    fn home_takes_precedence_over_userprofile() {
        let home = host_root("home");
        let windows_home = host_root("userprofile");
        let paths = Paths::from_env(env_map(&[
            ("HOME", env_value(&home)),
            ("USERPROFILE", env_value(&windows_home)),
        ]));
        assert_eq!(paths.config_dir(), home.join(".config/tinyharness"));
        assert_eq!(paths.home(), Some(home.as_path()));
    }

    #[test]
    fn relative_home_is_rejected() {
        let paths = Paths::from_env(env_map(&[("HOME", "relative/home")]));
        assert_eq!(paths.home(), None);
        assert_eq!(paths.config_dir(), abs(".config/tinyharness"));
    }

    #[test]
    fn relative_home_does_not_fall_through_to_userprofile() {
        // Locks the documented lookup order: HOME is preferred when it is
        // *set*, and the absolute check runs after the fallback, so an
        // unusable HOME suppresses home resolution entirely instead of
        // yielding to a perfectly usable USERPROFILE.
        let windows_home = host_root("userprofile");
        let paths = Paths::from_env(env_map(&[
            ("HOME", "relative/home"),
            ("USERPROFILE", env_value(&windows_home)),
        ]));
        assert_eq!(paths.home(), None);
        assert_eq!(paths.config_dir(), abs(".config/tinyharness"));
        assert_eq!(paths.data_dir(), abs(".local/share/tinyharness"));
    }

    #[test]
    fn with_home_derives_every_location() {
        let paths = Paths::with_home("/tmp/profile");
        assert_eq!(
            paths.settings_file(),
            abs("/tmp/profile/.config/tinyharness/settings.json")
        );
        assert_eq!(
            paths.prompts_dir(),
            abs("/tmp/profile/.config/tinyharness/prompts")
        );
        assert_eq!(
            paths.custom_tools_file(),
            abs("/tmp/profile/.config/tinyharness/custom_tools.json")
        );
        assert_eq!(
            paths.sessions_dir(),
            abs("/tmp/profile/.local/share/tinyharness/sessions")
        );
        assert_eq!(
            paths.history_dir(),
            abs("/tmp/profile/.local/share/tinyharness")
        );
        // project_dir seeds at the root so in-home projects still resolve
        assert_eq!(
            paths.project_config_root(),
            abs("/tmp/profile/.tinyharness")
        );
    }

    #[test]
    fn personal_skills_dirs_prefer_xdg_then_legacy() {
        let paths = Paths::with_home("/tmp/profile");
        assert_eq!(
            paths.personal_skills_dirs(),
            vec![
                abs("/tmp/profile/.config/tinyharness/skills"),
                abs("/tmp/profile/.tinyharness/skills"),
            ]
        );
    }

    #[test]
    fn personal_skills_dirs_dedupes_when_legacy_matches_config() {
        // A caller that points config_dir straight at the legacy dot-dir
        // should not have it scanned twice.
        let paths = Paths::with_home("/tmp/profile").with_config_dir("/tmp/profile/.tinyharness");
        assert_eq!(
            paths.personal_skills_dirs(),
            vec![abs("/tmp/profile/.tinyharness/skills")]
        );
    }

    #[test]
    fn personal_skills_dirs_omit_legacy_without_home() {
        let paths = Paths::new(abs("/cfg"), abs("/data"), abs("/proj"));
        assert_eq!(paths.personal_skills_dirs(), vec![abs("/cfg/skills")]);
        assert_eq!(paths.home(), None);
    }

    #[test]
    fn with_legacy_home_restores_legacy_scan_only() {
        let paths = Paths::new(abs("/cfg"), abs("/data"), abs("/proj")).with_legacy_home("/home/u");
        assert_eq!(paths.config_dir(), abs("/cfg"));
        assert_eq!(
            paths.personal_skills_dirs(),
            vec![abs("/cfg/skills"), abs("/home/u/.tinyharness/skills")]
        );
    }

    #[test]
    fn builders_override_individual_roots() {
        let paths = Paths::with_home("/tmp/profile")
            .with_config_dir("/custom/cfg")
            .with_data_dir("/custom/data")
            .with_project_dir("/custom/proj");
        assert_eq!(paths.settings_file(), abs("/custom/cfg/settings.json"));
        assert_eq!(paths.sessions_dir(), abs("/custom/data/sessions"));
        assert_eq!(
            paths.project_settings_file(),
            abs("/custom/proj/.tinyharness/config.json")
        );
        assert_eq!(
            paths.project_custom_tools_file(),
            abs("/custom/proj/.tinyharness/custom_tools.json")
        );
        assert_eq!(
            paths.project_skills_dir(),
            abs("/custom/proj/.tinyharness/skills")
        );
    }

    #[test]
    fn overrides_re_derive_dependent_paths() {
        // Guards against caching derived file paths at construction time.
        let base = Paths::with_home("/tmp/profile");
        let moved = base.clone().with_data_dir("/elsewhere/data");
        assert_ne!(base.sessions_dir(), moved.sessions_dir());
        assert_eq!(moved.sessions_dir(), abs("/elsewhere/data/sessions"));
    }

    #[test]
    fn default_reads_process_env() {
        // Only structural guarantees: whatever the ambient environment says,
        // the derived paths must agree with each other.
        let paths = Paths::default();
        assert!(paths.settings_file().ends_with("settings.json"));
        assert!(paths.sessions_dir().ends_with("sessions"));
        assert_eq!(
            paths.project_settings_file(),
            paths.project_dir().join(".tinyharness/config.json")
        );
        assert!(!paths.personal_skills_dirs().is_empty());
    }
}
