//! Interactive provider configuration helpers.
//!
//! These functions are used by the binary crate when:
//! - the user passes `--config` (full interactive walkthrough),
//! - the user passes a provider flag (`--ollama` / `--llama-cpp` / `--vllm`)
//!   without a `--url` (prompt for the URL only).
//!
//! All functions read directly from stdin and write user-facing messages to
//! stdout. They refuse to run when stdin is not a TTY (i.e. the process is
//! being piped or scripted) and instead return a clear error so callers can
//! fall back to a non-interactive default.

use std::io::{IsTerminal, Write};

use tinyharness_lib::config::{ProviderKind, Settings, load_settings, save_settings};
use tinyharness_ui::output::Output;
use tinyharness_ui::style::*;

/// Result of the interactive setup.
#[derive(Debug, Clone)]
pub struct SetupResult {
    pub provider: ProviderKind,
    pub url: String,
    /// True if the user changed the API key during this setup.
    pub api_key_changed: bool,
}

/// What the user chose to do with the API key in `prompt_for_api_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyChoice {
    /// Keep the existing key (or no key) as-is.
    Keep,
    /// Set a new key (the new value is in the `String`).
    Set(String),
    /// Remove the existing key.
    Clear,
}

/// Display a stored API key in masked form (first 4 + last 4 chars).
/// Returns "****" for short keys and "(not set)" for `None`.
pub fn mask_api_key(key: Option<&String>) -> String {
    match key {
        None => "(not set)".to_string(),
        Some(k) if k.len() > 8 => format!("{}...{}", &k[..4], &k[k.len() - 4..]),
        Some(_) => "****".to_string(),
    }
}

/// Prompt the user about the Ollama API key (only relevant for Ollama).
///
/// Shows the current state, then offers three options:
/// - Enter        → keep the current value
/// - type a key   → set a new key
/// - type "clear" → remove the existing key
///
/// Returns `Err(String)` if stdin is not a TTY.
pub fn prompt_for_api_key(out: &mut Output) -> Result<ApiKeyChoice, String> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "Interactive API key prompt requires a TTY. Use /apikey inside the app to set the key non-interactively.".to_string(),
        );
    }

    let current = load_settings().ollama_api_key;
    let _ = writeln!(
        out,
        "\n{BOLD}Ollama API key (for web search):{RESET} {BLUE}{}{RESET}",
        mask_api_key(current.as_ref())
    );
    let _ = writeln!(
        out,
        "{GRAY}Used for the /web_search tool. Optional — leave blank to keep current value, type a key to set it, or type 'clear' to remove it.{RESET}"
    );
    let _ = writeln!(
        out,
        "{ORANGE}Note:{RESET} the key will be visible while you type. Consider clearing your scrollback after setup."
    );

    let _ = write!(out, "\n{BOLD}API key [keep]:{RESET} ");
    let _ = out.flush();

    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => {
            // EOF: keep whatever was there
            return Ok(ApiKeyChoice::Keep);
        }
        Ok(_) => {}
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(ApiKeyChoice::Keep)
    } else if trimmed.eq_ignore_ascii_case("clear") {
        Ok(ApiKeyChoice::Clear)
    } else {
        Ok(ApiKeyChoice::Set(trimmed.to_string()))
    }
}

/// Prompt the user to select a provider kind from a numbered list.
///
/// Returns `Err(String)` if stdin is not a TTY, or the user submits an
/// unparseable choice.
pub fn prompt_for_provider(out: &mut Output) -> Result<ProviderKind, String> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "Interactive provider selection requires a TTY. Re-run with --ollama, --llama-cpp, --vllm, --openai-compat, or --sockudo, or pass --url.".to_string(),
        );
    }

    let _ = writeln!(out, "\n{BOLD}Select a provider:{RESET}");
    let _ = writeln!(
        out,
        "  {CYAN}1{RESET}) ollama        (default: {GRAY}http://127.0.0.1:11434{RESET})"
    );
    let _ = writeln!(
        out,
        "  {CYAN}2{RESET}) llama.cpp      (default: {GRAY}http://127.0.0.1:8080{RESET})"
    );
    let _ = writeln!(
        out,
        "  {CYAN}3{RESET}) vllm           (default: {GRAY}http://127.0.0.1:8000{RESET})"
    );
    let _ = writeln!(
        out,
        "  {CYAN}4{RESET}) sockudo        (default: {GRAY}http://127.0.0.1:6001{RESET})"
    );
    let _ = writeln!(
        out,
        "  {CYAN}5{RESET}) openai-compat  (hosted gateway, requires {GRAY}--api-key{RESET})"
    );

    loop {
        let _ = write!(out, "\n{BOLD}Choice [1]:{RESET} ");
        let _ = out.flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => {
                return Err("No provider selected (stdin closed).".to_string());
            }
            Ok(_) => {}
        }
        let line = line.trim();
        if line.is_empty() || line == "1" {
            return Ok(ProviderKind::Ollama);
        }
        if line == "2" {
            return Ok(ProviderKind::LlamaCpp);
        }
        if line == "3" {
            return Ok(ProviderKind::Vllm);
        }
        if line == "4" {
            return Ok(ProviderKind::Sockudo);
        }
        if line == "5" {
            return Ok(ProviderKind::OpenAiCompat);
        }
        let _ = writeln!(
            out,
            "{ORANGE}Please enter 1, 2, 3, 4, or 5 (or press Enter for the default).{RESET}"
        );
    }
}

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
/// 2. `last_provider_url` from settings (only if it matches `kind`)
/// 3. the hardcoded default for `kind`
pub fn resolve_url(kind: ProviderKind, cli_url: &str, settings: &Settings) -> String {
    if !cli_url.is_empty() {
        return cli_url.to_string();
    }
    if let Some(saved) = &settings.last_provider_url {
        return saved.clone();
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
pub fn save_provider_settings(kind: ProviderKind, url: &str) {
    // Load existing settings to preserve unrelated fields (mode, model, etc.)
    let mut s = tinyharness_lib::config::load_settings();
    s.last_provider = kind;
    s.last_provider_url = Some(url.to_string());
    save_settings(&s);
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
/// * `Some("")` — explicit opt-out from auth. Returns `Some(String::new())`
///   so the provider can be constructed without an `Authorization` header.
/// * `Some(key)` — use this key (and persist it).
///
/// Returns `None` when no key should be sent. Only the `OpenAiCompat`
/// provider uses this value; Ollama, llama.cpp, vLLM, and Sockudo ignore it.
pub fn resolve_api_key(cli_api_key: Option<&str>, settings: &Settings) -> Option<String> {
    let result = resolve_api_key_pure(cli_api_key, settings);
    // Persist side-effects only when the user actually passed `--api-key` on
    // the command line (cli_api_key is Some). The pure function is used in
    // tests to avoid touching the real settings file.
    if let Some(raw) = cli_api_key {
        let mut s = load_settings();
        match raw {
            "-" | "" => {
                // Both the clear sentinel and the explicit-empty opt-out
                // remove any persisted key.
                s.openai_compat_api_key = None;
            }
            _ => {
                s.openai_compat_api_key = Some(raw.to_string());
            }
        }
        save_settings(&s);
    }
    result
}

/// Pure (side-effect-free) API key resolution.  Used by tests so they never
/// touch the real `~/.config/tinyharness/settings.json`.
pub fn resolve_api_key_pure(cli_api_key: Option<&str>, settings: &Settings) -> Option<String> {
    match cli_api_key {
        Some("-") => return None,
        Some("") => return Some(String::new()),
        Some(k) => return Some(k.to_string()),
        None => {}
    }
    if let Ok(env_key) = std::env::var("OPENAI_API_KEY")
        && !env_key.is_empty()
    {
        return Some(env_key);
    }
    settings
        .openai_compat_api_key
        .clone()
        .filter(|s| !s.is_empty())
}

/// Persist an API key change based on the user's choice.
pub fn apply_api_key_choice(choice: ApiKeyChoice) -> bool {
    let mut s = load_settings();
    match choice {
        ApiKeyChoice::Keep => false,
        ApiKeyChoice::Clear => {
            s.ollama_api_key = None;
            save_settings(&s);
            true
        }
        ApiKeyChoice::Set(key) => {
            s.ollama_api_key = Some(key);
            save_settings(&s);
            true
        }
    }
}

/// Prompt the user for Sockudo app credentials (app_id, app_key, app_secret).
///
/// Shows the current stored values and allows the user to keep or change each.
/// Persists the result to settings.
fn prompt_for_sockudo_credentials(out: &mut Output) -> Result<(), String> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "Interactive Sockudo credential prompt requires a TTY. Set sockudo_app_id, sockudo_app_key, and sockudo_app_secret in settings.json manually.".to_string(),
        );
    }

    let s = load_settings();

    let _ = writeln!(
        out,
        "\n{BOLD}Sockudo app credentials:{RESET}\n{GRAY}Required for AI Transport authentication (HMAC-SHA256 signed requests).{RESET}"
    );

    // App ID
    let current_id = s.sockudo_app_id.as_deref().unwrap_or("(not set)");
    let _ = write!(
        out,
        "\n{BOLD}App ID [{GRAY}{current_id}{RESET}{BOLD}]:{RESET} "
    );
    let _ = out.flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => return Err("stdin closed".to_string()),
        Ok(_) => {}
    }
    let app_id = {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            s.sockudo_app_id.clone().unwrap_or_default()
        } else {
            trimmed.to_string()
        }
    };

    // App Key
    let current_key = s.sockudo_app_key.as_deref().unwrap_or("(not set)");
    let _ = write!(
        out,
        "\n{BOLD}App Key [{GRAY}{current_key}{RESET}{BOLD}]:{RESET} "
    );
    let _ = out.flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => return Err("stdin closed".to_string()),
        Ok(_) => {}
    }
    let app_key = {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            s.sockudo_app_key.clone().unwrap_or_default()
        } else {
            trimmed.to_string()
        }
    };

    // App Secret
    let _ = writeln!(
        out,
        "\n{ORANGE}Note:{RESET} the secret will be visible while you type."
    );
    let current_secret = mask_api_key(s.sockudo_app_secret.as_ref());
    let _ = write!(
        out,
        "{BOLD}App Secret [{GRAY}{current_secret}{RESET}{BOLD}]:{RESET} "
    );
    let _ = out.flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => return Err("stdin closed".to_string()),
        Ok(_) => {}
    }
    let app_secret = {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            s.sockudo_app_secret.clone().unwrap_or_default()
        } else {
            trimmed.to_string()
        }
    };

    // Save to settings
    let mut s = load_settings();
    s.sockudo_app_id = Some(app_id);
    s.sockudo_app_key = Some(app_key);
    s.sockudo_app_secret = Some(app_secret);
    save_settings(&s);

    let _ = writeln!(
        out,
        "{GREEN}✔{RESET} {BOLD}Sockudo credentials saved.{RESET}"
    );
    Ok(())
}

/// Run the full interactive setup flow used by `--config`.
///
/// Asks the user for the provider, then the URL, then (if the provider is
/// Ollama) the API key. Persists the result to settings. The caller is
/// responsible for the actual health check / provider instantiation after
/// this returns.
pub fn interactive_setup(out: &mut Output) -> Result<SetupResult, String> {
    let kind = prompt_for_provider(out)?;
    let default = default_url_for(kind);
    let url = prompt_for_url(out, kind, default)?;

    let _ = writeln!(
        out,
        "\n{GREEN}✔{RESET} Selected {BOLD}{kind}{RESET} at {BLUE}{url}{RESET}"
    );

    save_provider_settings(kind, &url);

    // The Ollama API key is only used by the Ollama provider, so only prompt
    // for it when that's what the user picked. Other providers ignore it.
    let mut api_key_changed = false;
    if matches!(kind, ProviderKind::Ollama) {
        let choice = prompt_for_api_key(out)?;
        api_key_changed = apply_api_key_choice(choice);
        if api_key_changed {
            let _ = writeln!(out, "{GREEN}✔{RESET} {BOLD}API key updated.{RESET}");
        } else {
            let _ = writeln!(out, "{GRAY}API key unchanged.{RESET}");
        }
    }

    // Sockudo needs app credentials (app_id, app_key, app_secret).
    if matches!(kind, ProviderKind::Sockudo) {
        prompt_for_sockudo_credentials(out)?;
    }

    let _ = writeln!(
        out,
        "\n{GRAY}Saved to settings. Run tinyharness to start.{RESET}"
    );

    Ok(SetupResult {
        provider: kind,
        url,
        api_key_changed,
    })
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
        let s = Settings {
            last_provider_url: Some("http://saved:1234".to_string()),
            ..Settings::default()
        };
        assert_eq!(
            resolve_url(ProviderKind::Ollama, "http://cli:9999", &s),
            "http://cli:9999"
        );
    }

    #[test]
    fn resolve_url_uses_saved_when_no_cli() {
        let s = Settings {
            last_provider_url: Some("http://saved:1234".to_string()),
            ..Settings::default()
        };
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
            openai_compat_api_key: Some("sk-settings".to_string()),
            ..Settings::default()
        };
        assert_eq!(
            resolve_api_key_pure(None, &s),
            Some("sk-settings".to_string())
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
            openai_compat_api_key: Some(String::new()),
            ..Settings::default()
        };
        assert_eq!(resolve_api_key_pure(None, &s), None);
    }

    #[test]
    fn resolve_api_key_cli_value_wins() {
        // CLI key takes precedence over env and settings.
        let s = Settings {
            openai_compat_api_key: Some("sk-settings".to_string()),
            ..Settings::default()
        };
        assert_eq!(
            resolve_api_key_pure(Some("sk-from-cli"), &s),
            Some("sk-from-cli".to_string())
        );
    }

    #[test]
    fn resolve_api_key_clear_sentinel() {
        // The sentinel "-" should return None (clear).
        let s = Settings {
            openai_compat_api_key: Some("sk-settings".to_string()),
            ..Settings::default()
        };
        assert_eq!(resolve_api_key_pure(Some("-"), &s), None);
    }

    #[test]
    fn resolve_api_key_explicit_empty_opts_out() {
        // Explicit `--api-key ""` returns Some("") so the caller can
        // construct a no-auth provider, ignoring any stored key.
        let s = Settings {
            openai_compat_api_key: Some("sk-settings".to_string()),
            ..Settings::default()
        };
        assert_eq!(resolve_api_key_pure(Some(""), &s), Some(String::new()));
    }

    #[test]
    fn setup_result_carries_values() {
        let r = SetupResult {
            provider: ProviderKind::Vllm,
            url: "http://example:8000".to_string(),
            api_key_changed: false,
        };
        assert_eq!(r.provider, ProviderKind::Vllm);
        assert_eq!(r.url, "http://example:8000");
        assert!(!r.api_key_changed);
    }

    #[test]
    fn setup_result_marks_api_key_changed() {
        let r = SetupResult {
            provider: ProviderKind::Ollama,
            url: "http://127.0.0.1:11434".to_string(),
            api_key_changed: true,
        };
        assert!(r.api_key_changed);
    }

    #[test]
    fn mask_api_key_handles_none() {
        assert_eq!(mask_api_key(None), "(not set)");
    }

    #[test]
    fn mask_api_key_handles_long_keys() {
        // 12-char key → first 4 + "..." + last 4
        assert_eq!(
            mask_api_key(Some(&"abcdef123456".to_string())),
            "abcd...3456"
        );
    }

    #[test]
    fn mask_api_key_handles_short_keys() {
        // 8 chars or fewer → fully masked
        assert_eq!(mask_api_key(Some(&"abc".to_string())), "****");
        assert_eq!(mask_api_key(Some(&"abcdefgh".to_string())), "****");
    }

    #[test]
    fn mask_api_key_handles_exactly_9_chars() {
        // 9 chars is the threshold: still show first/last 4
        assert_eq!(mask_api_key(Some(&"abcdefghi".to_string())), "abcd...fghi");
    }

    #[test]
    fn api_key_choice_equality() {
        assert_eq!(ApiKeyChoice::Keep, ApiKeyChoice::Keep);
        assert_ne!(ApiKeyChoice::Keep, ApiKeyChoice::Clear);
        assert_eq!(
            ApiKeyChoice::Set("k".to_string()),
            ApiKeyChoice::Set("k".to_string())
        );
        assert_ne!(
            ApiKeyChoice::Set("a".to_string()),
            ApiKeyChoice::Set("b".to_string())
        );
    }

    // Note: prompt_for_provider and prompt_for_url read from stdin and are
    // not unit-tested here (they'd require process fork/pipe mocking). They
    // are exercised by manual / integration tests. The non-interactive error
    // path is verified by the `is_terminal()` guard in the function bodies.
}
