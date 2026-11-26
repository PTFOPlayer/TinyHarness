//! Provider resolution helpers: URL prompting, URL/API-key precedence.
//!
//! Used by the binary crate when a provider flag is passed without a `--url` —
//! we prompt for the URL only. There is intentionally no interactive setup
//! wizard; `docs/configuration.md` covers configuration, and Sockudo
//! credentials are settings-file-only.
//!
//! `prompt_for_url` reads stdin and refuses to run when stdin is not a TTY, so
//! scripted callers get a clear error instead of a hang.

use std::io::{IsTerminal, Write};

use tinyharness_lib::SecretString;
use tinyharness_lib::config::{ProviderKind, Settings, update_settings_from};
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
    update_settings_from(paths, |s| {
        s.last_provider = kind;
        s.set_url_for(kind, url.to_string());
    });
}

/// Resolve the bearer token for the provider selected on the command line.
///
/// This is the CLI/settings entry point. `send_auth` decides whether the
/// provider actually puts a credential on the wire; for every kind other than
/// `--openai-compat` it is `false`, which means (a) no key is returned and
/// (b) `--api-key` is not persisted, because storing a token for a backend
/// that never reads it would silently authenticate the wrong provider later.
///
/// Precedence when auth is active:
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
/// Returns `None` when no key should be sent. Ollama, llama.cpp, vLLM, and
/// Sockudo ignore the value — see [`provider_sends_auth`].
pub fn resolve_api_key(
    paths: &tinyharness_lib::Paths,
    cli_api_key: Option<&str>,
    settings: &Settings,
    send_auth: bool,
) -> Option<SecretString> {
    // The flag's *presence* is what may authorize a persisted key: a sentinel
    // such as `""` must not be swallowed by the policy gate.
    if !send_auth {
        // A key passed for a backend that does not authenticate is discarded,
        // and nothing is persisted.
        if cli_api_key.is_some() {
            tracing::warn!(
                "`--api-key` was passed but the active provider is not the \
                 OpenAI-compatible provider: no credential is sent and none is saved"
            );
        }
        return None;
    }
    let result = resolve_api_key_pure(cli_api_key, settings);
    // Persist side-effects only when the user actually passed `--api-key` on
    // the command line (cli_api_key is Some). The pure function is used in
    // tests to avoid touching the real settings file.
    if let Some(raw) = cli_api_key {
        update_settings_from(paths, |s| match raw {
            "-" | "" => {
                // Both the clear sentinel and the explicit-empty opt-out
                // remove any persisted key.
                s.openai_compat_api_key = None;
            }
            _ => {
                s.openai_compat_api_key = Some(SecretString::new(raw));
            }
        });
    }
    result
}

/// Pure (side-effect-free) API key resolution.  Used by tests so they never
/// touch the real `<config_dir>/settings.json`.
///
/// `env_key` is injected rather than read here so the precedence rule can be
/// tested without mutating the process environment.
pub fn resolve_api_key_pure_with(
    cli_api_key: Option<&str>,
    env_key: Option<&str>,
    settings: &Settings,
) -> Option<SecretString> {
    match cli_api_key {
        Some("-") => return None,
        Some("") => return Some(SecretString::default()),
        Some(k) => return Some(SecretString::new(k)),
        None => {}
    }
    if let Some(env_key) = env_key.filter(|k| !k.is_empty()) {
        return Some(SecretString::new(env_key));
    }
    settings
        .openai_compat_api_key
        .clone()
        .filter(|s| !s.is_empty())
}

/// [`resolve_api_key_pure_with`] reading the real `OPENAI_API_KEY`.
pub fn resolve_api_key_pure(
    cli_api_key: Option<&str>,
    settings: &Settings,
) -> Option<SecretString> {
    resolve_api_key_pure_with(
        cli_api_key,
        std::env::var("OPENAI_API_KEY").ok().as_deref(),
        settings,
    )
}

/// Whether `kind` sends a bearer token on its requests.
///
/// `OpenAiCompat` is the only backend that is actually authenticated: the
/// OpenAI-compatible surface (OpenRouter / Together / Groq / custom gateways)
/// requires it. `Ollama`, `llama.cpp`, and `vLLM` bind a plain-HTTP local
/// socket and send no credential, so a saved or env `OPENAI_API_KEY` is never
/// attached to them — passing a key for one of them is silently dropped.
///
/// `Sockudo` authenticates with its own app credentials in settings
/// (`sockudo_app_key` / `sockudo_app_secret`), never with this value.
pub fn provider_sends_auth(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::OpenAiCompat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinyharness_lib::config::load_settings_from;

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
        assert_eq!(
            resolve_api_key_pure_with(None, None, &s),
            None,
            "no provider gets an auth value for free"
        );
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

    // ── Auth policy per provider ──

    /// `OPENAI_API_KEY` is only consulted by the OpenAI-compatible provider.
    /// Ollama, llama.cpp, and vLLM talk to a plain-HTTP local socket and must
    /// never inherit a token from another tool's environment.
    #[test]
    fn only_openai_compat_sends_a_bearer_token() {
        for kind in [
            ProviderKind::Ollama,
            ProviderKind::LlamaCpp,
            ProviderKind::Vllm,
            ProviderKind::OpenAiCompat,
            ProviderKind::Sockudo,
        ] {
            assert_eq!(
                provider_sends_auth(kind),
                matches!(kind, ProviderKind::OpenAiCompat),
                "{kind} must have a well-defined auth policy"
            );
        }
    }

    /// Regression test for "a saved key silently authenticates the wrong
    /// provider": the resolved value is only attached to the requests of a
    /// kind that declares it sends auth.
    #[test]
    fn env_key_not_attached_to_local_providers() {
        // Resolution itself is provider-agnostic ...
        let s = Settings::default();
        assert_eq!(
            resolve_api_key_pure_with(None, Some("sk-from-env"), &s),
            Some(SecretString::new("sk-from-env"))
        );
        // ... and the active kind gates it. `AnyProvider::build` passes `None`
        // for every local backend, so a resolved key can never leak into an
        // Ollama / llama.cpp / vLLM request.
        for kind in [
            ProviderKind::Ollama,
            ProviderKind::LlamaCpp,
            ProviderKind::Vllm,
            ProviderKind::Sockudo,
        ] {
            assert!(
                !provider_sends_auth(kind),
                "{kind} must not attach a bearer token"
            );
        }
        assert!(provider_sends_auth(ProviderKind::OpenAiCompat));
    }

    /// The env var must be checked but never beat an explicit `--api-key ""`
    /// opt-out (which means "this gateway is unauthenticated").
    #[test]
    fn explicit_empty_beats_env_key() {
        let s = Settings::default();
        assert_eq!(
            resolve_api_key_pure_with(Some(""), Some("sk-from-env"), &s),
            Some(SecretString::default())
        );
        assert_eq!(
            resolve_api_key_pure_with(Some("-"), Some("sk-from-env"), &s),
            None
        );
    }

    /// An empty env var counts as unset (falls through to settings).
    #[test]
    fn empty_env_var_falls_through_to_settings() {
        let s = Settings {
            openai_compat_api_key: Some(SecretString::new("sk-settings")),
            ..Settings::default()
        };
        assert_eq!(
            resolve_api_key_pure_with(None, Some(""), &s),
            Some(SecretString::new("sk-settings"))
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

    #[test]
    fn local_backends_are_never_built_with_a_credential() {
        // Mirrors the gate in `create_provider`: only `OpenAiCompat` may be
        // handed a resolved key, so a value read from `OPENAI_API_KEY` (or
        // persisted in settings) can never reach a local or Sockudo backend.
        let resolved = Some(SecretString::new("sk-from-env"));
        for kind in [
            ProviderKind::Ollama,
            ProviderKind::LlamaCpp,
            ProviderKind::Vllm,
            ProviderKind::OpenAiCompat,
            ProviderKind::Sockudo,
        ] {
            let passed = if provider_sends_auth(kind) {
                resolved.clone()
            } else {
                None
            };
            assert_eq!(
                passed.is_some(),
                kind == ProviderKind::OpenAiCompat,
                "{kind} must not receive the credential"
            );
        }
    }

    #[test]
    fn api_key_persisted_for_openai_compat() {
        let paths = crate::test_helpers::isolated_paths("setup-auth-gate");
        resolve_api_key(&paths, Some("sk-from-cli"), &Settings::default(), true);
        assert_eq!(
            load_settings_from(&paths).openai_compat_api_key,
            Some(SecretString::new("sk-from-cli"))
        );
    }

    /// Regression test: `--api-key` passed for a local backend must not be
    /// persisted, because a token stored for one provider silently becomes the
    /// credential for the next `--openai-compat` launch.
    #[test]
    fn api_key_for_local_backend_is_not_persisted() {
        let paths = crate::test_helpers::isolated_paths("setup-auth-gate-local");
        // A key already stored for a hosted gateway must survive an unrelated run.
        update_settings_from(&paths, |s| {
            s.openai_compat_api_key = Some(SecretString::new("sk-hosted"));
        });
        let before = load_settings_from(&paths).openai_compat_api_key;

        let key = resolve_api_key(&paths, Some("sk-local"), &Settings::default(), false);

        assert_eq!(
            key, None,
            "the local backend must not receive the resolved key"
        );
        assert_eq!(
            load_settings_from(&paths).openai_compat_api_key,
            before,
            "an unrelated run must not overwrite the hosted-gateway key"
        );
    }

    #[test]
    fn api_key_local_backend_does_not_clear_hosted_key() {
        let paths = crate::test_helpers::isolated_paths("setup-auth-gate-clear");
        update_settings_from(&paths, |s| {
            s.openai_compat_api_key = Some(SecretString::new("sk-hosted"));
        });
        // An explicit clear sentinel is still honoured (it's the documented way
        // to forget a key), but only for the provider that reads the key.
        resolve_api_key(&paths, Some("-"), &Settings::default(), false);
        assert_eq!(
            load_settings_from(&paths).openai_compat_api_key,
            Some(SecretString::new("sk-hosted")),
            "the clear sentinel must not be applied to a backend that ignores the flag"
        );
    }

    // ── Persisting helpers — isolated root, never the real profile ──

    #[test]
    fn resolve_api_key_persists_to_injected_paths() {
        let paths = crate::test_helpers::isolated_paths("setup");
        let key = resolve_api_key(&paths, Some("sk-from-cli"), &Settings::default(), true);
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
        resolve_api_key(&paths, Some("sk-stored"), &Settings::default(), true);
        resolve_api_key(&paths, Some("-"), &Settings::default(), true);
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
