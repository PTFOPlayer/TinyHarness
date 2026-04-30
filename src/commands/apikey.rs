use crate::config::Settings;
use crate::style::*;

pub fn execute_set(key: &str) {
    let mut settings = Settings::load();
    settings.ollama_api_key = Some(key.to_string());
    settings.save();
    println!(
        "{}Ollama API key saved.{}",
        BOLD, RESET
    );
}

pub fn execute_show() {
    let settings = Settings::load();
    match &settings.ollama_api_key {
        Some(key) => {
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            println!(
                "{}Ollama API key:{} {}",
                BOLD, RESET, masked
            );
        }
        None => println!(
            "{}No Ollama API key set.{} Use {}/apikey <key>{} to set one.",
            ORANGE, RESET, BLUE, RESET
        ),
    }
}

pub fn execute_clear() {
    let mut settings = Settings::load();
    settings.ollama_api_key = None;
    settings.save();
    println!(
        "{}Ollama API key cleared.{}",
        BOLD, RESET
    );
}
