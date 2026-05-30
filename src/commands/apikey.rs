use std::io::Write;

use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_ui::output::Output;

use crate::style::*;

pub fn execute_set(out: &mut Output, key: &str) {
    let mut settings = load_settings();
    settings.ollama_api_key = Some(key.to_string());
    save_settings(&settings);
    let _ = writeln!(out, "{BOLD}Ollama API key saved.{RESET}");
}

pub fn execute_show(out: &mut Output) {
    let settings = load_settings();
    match &settings.ollama_api_key {
        Some(key) => {
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            let _ = writeln!(out, "{BOLD}Ollama API key:{RESET} {masked}");
        }
        None => {
            let _ = writeln!(
                out,
                "{ORANGE}No Ollama API key set.{RESET} Use {BLUE}/apikey <key>{RESET} to set one.",
            );
        }
    }
}

pub fn execute_clear(out: &mut Output) {
    let mut settings = load_settings();
    settings.ollama_api_key = None;
    save_settings(&settings);
    let _ = writeln!(out, "{BOLD}Ollama API key cleared.{RESET}");
}
