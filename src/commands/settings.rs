use crate::config::Settings;
use crate::style::*;

pub fn execute() {
    let settings = Settings::load();

    println!();
    println!(
        "{}╭─ Settings ─────────────────────────────────╮{}",
        BOLD, RESET
    );

    let provider_str = format!("{}", settings.last_provider);
    println!(
        "{}│{} Provider:  {}{}{}",
        BOLD, RESET, BLUE, provider_str, RESET
    );

    match &settings.last_model {
        Some(model) => println!("{}│{} Model:     {}{}{}", BOLD, RESET, BLUE, model, RESET),
        None => println!("{}│{} Model:     {}none{}", BOLD, RESET, ORANGE, RESET),
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
            println!("{}│{} API Key:   {}{}{}", BOLD, RESET, BLUE, masked, RESET);
        }
        None => println!("{}│{} API Key:   {}not set{}", BOLD, RESET, ORANGE, RESET),
    }

    println!(
        "{}│{} Timeout:   {}{}s{}",
        BOLD, RESET, BLUE, settings.ollama_timeout_secs, RESET
    );
    println!(
        "{}│{} Retries:   {}{}{}",
        BOLD, RESET, BLUE, settings.ollama_max_retries, RESET
    );

    match settings.context_limit {
        Some(limit) => println!(
            "{}│{} Ctx Limit: {}{} tokens{}",
            BOLD, RESET, BLUE, limit, RESET
        ),
        None => println!(
            "{}│{} Ctx Limit: {}auto (model default){}",
            BOLD, RESET, GRAY, RESET
        ),
    }

    println!(
        "{}╰────────────────────────────────────────────╯{}",
        BOLD, RESET
    );
    println!();
}
