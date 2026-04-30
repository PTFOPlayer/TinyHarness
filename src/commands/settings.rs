use crate::config::Settings;
use crate::style::*;

pub fn execute() {
    let settings = Settings::load();

    println!();
    println!("{}╭─ Settings ─────────────────────────────────╮{}", BOLD, RESET);

    let provider_str = format!("{}", settings.last_provider);
    println!(
        "{}│{} Provider:  {}{}{}",
        BOLD, RESET, BLUE, provider_str, RESET
    );

    match &settings.last_model {
        Some(model) => println!(
            "{}│{} Model:     {}{}{}",
            BOLD, RESET, BLUE, model, RESET
        ),
        None => println!(
            "{}│{} Model:     {}none{}",
            BOLD, RESET, ORANGE, RESET
        ),
    }

    println!(
        "{}│{} Mode:      {}{}{}",
        BOLD, RESET, BLUE, settings.preferred_mode, RESET
    );

    match &settings.ollama_api_key {
        Some(key) => {
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            println!(
                "{}│{} API Key:   {}{}{}",
                BOLD, RESET, BLUE, masked, RESET
            );
        }
        None => println!(
            "{}│{} API Key:   {}not set{}",
            BOLD, RESET, ORANGE, RESET
        ),
    }

    println!("{}╰────────────────────────────────────────────╯{}", BOLD, RESET);
    println!();
}
