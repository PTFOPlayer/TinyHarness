use crate::provider::Provider;
use crate::style::*;

pub async fn execute_list(provider: &dyn Provider) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.is_empty() {
        println!("{}No models available.{}", ORANGE, RESET);
    } else {
        println!("\n{}Available models:{}", BOLD, RESET);
        for model in &models {
            println!("  {}{}{}", BLUE, model, RESET);
        }
        println!();
    }
    Ok(())
}

pub async fn execute_select(provider: &mut dyn Provider, name: &str) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.iter().any(|m| m == name) {
        provider.select_model(name.to_string());
        println!("{}Switched to model: {}{}{}", BOLD, BLUE, name, RESET);
        Ok(())
    } else {
        // Still switch even if not in list (model might be pullable)
        provider.select_model(name.to_string());
        println!("{}Set model to: {}{}{}", BOLD, BLUE, name, RESET);
        Ok(())
    }
}
