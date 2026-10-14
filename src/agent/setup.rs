//! Provider resolution helpers: URL prompting, URL/API-key precedence.
//!
//! Used by the binary crate when the user passes a provider flag
//! (`--ollama` / `--llama-cpp` / `--vllm` / `--sockudo` / `--openai-compat`)
//! without a `--url` — we prompt for the URL only.
//!
//! There is intentionally no interactive setup wizard: configuration is covered
//! by `docs/configuration.md`, and every setting is reachable via CLI flags or
//! in-app slash commands (`/model`, `/apikey`, `/timeout`, `/retries`,
//! `/contextlimit`, …). Sockudo credentials are settings-file-only.
//!
//! `prompt_for_url` reads directly from stdin and writes user-facing messages to
//! stdout. It refuses to run when stdin is not a TTY (i.e. the process is being
//! piped or scripted) and instead returns a clear error so callers can fall back
//! to a non-interactive default.

use std::io::{IsTerminal, Write};

use tinyharness_lib::SecretString;
use tinyharness_lib::config::{ProviderKind, Settings, load_settings_from, save_settings_to};
use tinyharness_ui::output::Output;
use tinyharness_ui::style::*;

/// Prompt the user for a URL, showing the default. Enter accepts the default.
///
/// Returns `Err(String)` if stdin is not a TTY, or the URL is empty after
/// trimming and there is no default.
pub fn prompt_for_url(
    out: &mut Output,
    kind: ProviderKind,
    default: &str,
) -> Result<String, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "Interactive URL prompt requires a TTY. Pass --url explicitly when running non-interactively (e.g. --{} <url>).",
            match kind {
                ProviderKind::Ollama => "ollama",
                ProviderKind::LlamaCpp => "llama-cpp",
                ProviderKind::Vllm => "vllm",
                ProviderKind::OpenAiCompat => "openai-compat",
                ProviderKind::Sockudo => "sockudo",
            }
        ));
    }

    let _ = write!(
        out,
        "\n{BOLD}URL for {kind} [{GRAY}{default}{RESET}{BOLD}]:{RESET} "
    );
    let _ = out.flush();

    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => {
            // EOF: use the default
            return Ok(default.to_string());
        }
        Ok(_) => {}
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Resolve the URL to use for the given provider based on (in order of
/// precedence):
/// 1. `cli_url` (the value of `--url` if passed)
/// 2. URL from `settings.provider_urls` for the given `kind`
/// 3. the hardcoded default for `kind`
pub fn resolve_url(kind: ProviderKind, cli_url: &str, settings: &Settings) -> String {
    if !cli_url.is_empty() {
        return cli_url.to_string();
    }
    if let Some(saved) = settings.get_url_for(kind) {
        return saved.to_string();
    }
    default_url_for(kind).to_string()
}

/// Hardcoded default URL for a provider (kept here as well as in main.rs so
/// this module is self-contained and testable).
pub fn default_url_for(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Ollama => "http://127.0.0.1:11434",
        ProviderKind::LlamaCpp => "http://127.0.0.1:8080",
        ProviderKind::Vllm => "http://127.0.0.1:8000",
        // No default — hosted gateways require an explicit URL.
        ProviderKind::OpenAiCompat => "",
        ProviderKind::Sockudo => "http://127.0.0.1:6001",
    }
}

/// Persist the provider + URL pair to settings.
///
/// `paths` is the process-wide [`Paths`](tinyharness_lib::Paths) built at
/// startup — `main.rs` calls this before a `CommandContext` exists, so the
/// instance is threaded in explicitly rather than read from the ambient
/// environment.
pub fn save_provider_settings(paths: &tinyharness_lib::Paths, kind: ProviderKind, url: &str) {
    // Load existing settings to preserve unrelated fields (mode, model, etc.)
    let mut s = load_settings_from(paths);
    s.last_provider = kind;
    s.set_url_for(kind, url.to_string());
    if let Err(e) = save_settings_to(paths, &s) {
        tracing::warn!("Failed to save settings: {e}");
    }
}

/// Resolve the bearer token for the OpenAI-compatible provider using the
/// precedence:
///
/// 1. `cli_api_key` — the value of `--api-key` if passed:
///    - A non-empty value (other than `-`) is used as the key and persisted
///      to settings so subsequent launches pick it up automatically.
///    - The sentinel value `"-"` clears any saved key and returns `None`.
///    - An empty string means the flag was not provided; fall through.
/// 2. `OPENAI_API_KEY` environment variable, if set and non-empty.
/// 3. `settings.openai_compat_api_key`, if set and non-empty.
///
/// `cli_api_key` semantics:
/// * `None` — flag not passed; fall through to env/settings.
/// * `Some("-")` — sentinel that clears any saved key. Returns `None`.
/// * `Some("")` — explicit opt-out from auth. Returns `Some(SecretString::default())`
///   so the provider can be constructed without an `Authorization` header.
/// * `Some(key)` — use this key (and persist it).
///
/// Returns `None` when no key should be sent. Only the `OpenAiCompat`
/// provider uses this value; Ollama, llama.cpp, vLLM, and Sockudo ignore it.
pub fn resolve_api_key(
    paths: &tinyharness_lib::Paths,
    cli_api_key: Option<&str>,
    settings: &Settings,
) -> Option<SecretString> {
    let result = resolve_api_key_pure(cli_api_key, settings);
    // Persist side-effects only when the user actually passed `--api-key` on
    // the command line (cli_api_key is Some). The pure function is used in
    // tests to avoid touching the real settings file.
    if let Some(raw) = cli_api_key {
        let mut s = load_settings_from(paths);
        match raw {
            "-" | "" => {
                // Both the clear sentinel and the explicit-empty opt-out
                // remove any persisted key.
                s.openai_compat_api_key = None;
            }
            _ => {
                s.openai_compat_api_key = Some(SecretString::new(raw));
            }
        }
        if let Err(e) = save_settings_to(paths, &s) {
            tracing::warn!("Failed to save settings: {e}");
        }
    }
    result
}

/// Pure (side-effect-free) API key resolution.  Used by tests so they never
/// touch the real `<config_dir>/settings.json`.
pub fn resolve_api_key_pure(
    cli_api_key: Option<&str>,
    settings: &Settings,
) -> Option<SecretString> {
    match cli_api_key {
        Some("-") => return None,
        Some("") => return Some(SecretString::default()),
        Some(k) => return Some(SecretString::new(k)),
        None => {}
    }
    if let Ok(env_key) = std::env::var("OPENAI_API_KEY")
        && !env_key.is_empty()
    {
        return Some(SecretString::new(env_key));
    }
    settings
        .openai_compat_api_key
        .clone()
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_url_for_each_provider() {
        assert_eq!(
            default_url_for(ProviderKind::Ollama),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            default_url_for(ProviderKind::LlamaCpp),
            "http://127.0.0.1:8080"
        );
        assert_eq!(default_url_for(ProviderKind::Vllm), "http://127.0.0.1:8000");
        assert_eq!(default_url_for(ProviderKind::OpenAiCompat), "");
        assert_eq!(
            default_url_for(ProviderKind::Sockudo),
            "http://127.0.0.1:6001"
        );
    }

    #[test]
    fn resolve_url_cli_overrides_everything() {
        let mut s = Settings::default();
        s.set_url_for(ProviderKind::Ollama, "http://saved:1234".to_string());
        assert_eq!(
            resolve_url(ProviderKind::Ollama, "http://cli:9999", &s),
            "http://cli:9999"
        );
    }

    #[test]
    fn resolve_url_uses_saved_when_no_cli() {
        let mut s = Settings::default();
        s.set_url_for(ProviderKind::Ollama, "http://saved:1234".to_string());
        assert_eq!(
            resolve_url(ProviderKind::Ollama, "", &s),
            "http://saved:1234"
        );
    }

    #[test]
    fn resolve_url_falls_back_to_default() {
        let s = Settings::default();
        assert_eq!(
            resolve_url(ProviderKind::Ollama, "", &s),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            resolve_url(ProviderKind::LlamaCpp, "", &s),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            resolve_url(ProviderKind::Vllm, "", &s),
            "http://127.0.0.1:8000"
        );
        // OpenAiCompat has no default URL — returns empty string, caller
        // should require --url.
        assert_eq!(resolve_url(ProviderKind::OpenAiCompat, "", &s), "");
        assert_eq!(
            resolve_url(ProviderKind::Sockudo, "", &s),
            "http://127.0.0.1:6001"
        );
    }

    #[test]
    fn resolve_api_key_settings_fallback() {
        // When no CLI flag and no env var, fall back to settings.
        let s = Settings {
            openai_compat_api_key: Some(SecretString::new("sk-settings")),
            ..Settings::default()
        };
        assert_eq!(
            resolve_api_key_pure(None, &s),
            Some(SecretString::new("sk-settings"))
        );
    }

    #[test]
    fn resolve_api_key_empty_settings_returns_none() {
        let s = Settings::default();
        assert_eq!(resolve_api_key_pure(None, &s), None);
    }

    #[test]
    fn resolve_api_key_empty_string_in_settings_is_ignored() {
        let s = Settings {
            openai_compat_api_key: Some(SecretString::default()),
            ..Settings::default()
        };
        assert_eq!(resolve_api_key_pure(None, &s), None);
    }

    #[test]
    fn resolve_api_key_cli_value_wins() {
        // CLI key takes precedence over env and settings.
        let s = Settings {
            openai_compat_api_key: Some(SecretString::new("sk-settings")),
            ..Settings::default()
        };
        assert_eq!(
            resolve_api_key_pure(Some("sk-from-cli"), &s),
            Some(SecretString::new("sk-from-cli"))
        );
    }

    #[test]
    fn resolve_api_key_clear_sentinel() {
        // The sentinel "-" should return None (clear).
        let s = Settings {
            openai_compat_api_key: Some(SecretString::new("sk-settings")),
            ..Settings::default()
        };
        assert_eq!(resolve_api_key_pure(Some("-"), &s), None);
    }

    #[test]
    fn resolve_api_key_explicit_empty_opts_out() {
        // Explicit `--api-key ""` returns Some("") so the caller can
        // construct a no-auth provider, ignoring any stored key.
        let s = Settings {
            openai_compat_api_key: Some(SecretString::new("sk-settings")),
            ..Settings::default()
        };
        assert_eq!(
            resolve_api_key_pure(Some(""), &s),
            Some(SecretString::default())
        );
    }

    // ── Impure (persisting) variants — isolated root, never the real profile ──

    #[test]
    fn resolve_api_key_persists_to_injected_paths() {
        let paths = crate::test_helpers::isolated_paths("setup");
        let key = resolve_api_key(&paths, Some("sk-from-cli"), &Settings::default());
        assert_eq!(key, Some(SecretString::new("sk-from-cli")));
        assert_eq!(
            load_settings_from(&paths).openai_compat_api_key,
            Some(SecretString::new("sk-from-cli")),
            "the CLI key must land in the injected settings file"
        );
    }

    #[test]
    fn resolve_api_key_sentinel_clears_saved_key_in_injected_paths() {
        let paths = crate::test_helpers::isolated_paths("setup");
        resolve_api_key(&paths, Some("sk-stored"), &Settings::default());
        resolve_api_key(&paths, Some("-"), &Settings::default());
        assert_eq!(load_settings_from(&paths).openai_compat_api_key, None);
    }

    #[test]
    fn save_provider_settings_writes_to_injected_paths() {
        let paths = crate::test_helpers::isolated_paths("setup");
        save_provider_settings(&paths, ProviderKind::LlamaCpp, "http://saved:8080");
        let saved = load_settings_from(&paths);
        assert_eq!(saved.last_provider, ProviderKind::LlamaCpp);
        assert_eq!(
            saved.get_url_for(ProviderKind::LlamaCpp),
            Some("http://saved:8080")
        );
    }

    // Note: prompt_for_url reads from stdin and is not unit-tested here (it
    // would require process fork/pipe mocking). It is exercised by manual /
    // integration tests. The non-interactive error path is verified by the
    // `is_terminal()` guard in the function body.
}
