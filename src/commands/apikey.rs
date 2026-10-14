use std::io::Write;

use tinyharness_lib::Paths;
use tinyharness_lib::SecretString;
use tinyharness_lib::config::{load_settings_from, save_settings_to};
use tinyharness_ui::output::Output;

use tinyharness_ui::style::*;

pub fn execute_set(out: &mut Output, paths: &Paths, key: &str) {
    let mut settings = load_settings_from(paths);
    settings.ollama_api_key = Some(SecretString::new(key));
    if let Err(e) = save_settings_to(paths, &settings) {
        tracing::warn!("Failed to save settings: {e}");
    }
    let _ = writeln!(out, "{BOLD}Ollama API key saved.{RESET}");
}

pub fn execute_show(out: &mut Output, paths: &Paths) {
    let settings = load_settings_from(paths);
    match &settings.ollama_api_key {
        Some(key) => {
            let _ = writeln!(out, "{BOLD}Ollama API key:{RESET} {}", key.masked());
        }
        None => {
            let _ = writeln!(
                out,
                "{ORANGE}No Ollama API key set.{RESET} Use {BLUE}/apikey <key>{RESET} to set one.",
            );
        }
    }
}

pub fn execute_clear(out: &mut Output, paths: &Paths) {
    let mut settings = load_settings_from(paths);
    settings.ollama_api_key = None;
    if let Err(e) = save_settings_to(paths, &settings) {
        tracing::warn!("Failed to save settings: {e}");
    }
    let _ = writeln!(out, "{BOLD}Ollama API key cleared.{RESET}");
}
