use std::io::Write;

use tinyharness_lib::SecretString;
use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_ui::output::Output;

use tinyharness_ui::style::*;

pub fn execute_set(out: &mut Output, key: &str) {
    let mut settings = load_settings();
    settings.ollama_api_key = Some(SecretString::new(key));
    save_settings(&settings);
    let _ = writeln!(out, "{BOLD}Ollama API key saved.{RESET}");
}

pub fn execute_show(out: &mut Output) {
    let settings = load_settings();
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

pub fn execute_clear(out: &mut Output) {
    let mut settings = load_settings();
    settings.ollama_api_key = None;
    save_settings(&settings);
    let _ = writeln!(out, "{BOLD}Ollama API key cleared.{RESET}");
}
