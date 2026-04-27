use crate::provider::Provider;

pub async fn execute_list(provider: &dyn Provider) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.is_empty() {
        println!("{}No models available.{}", crate::ORANGE, crate::RESET);
    } else {
        println!("\n{}Available models:{}", crate::BOLD, crate::RESET);
        for model in &models {
            println!("  {}{}{}", crate::BLUE, model, crate::RESET);
        }
        println!();
    }
    Ok(())
}

pub async fn execute_select(provider: &mut dyn Provider, name: &str) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.iter().any(|m| m == name) {
        provider.select_model(name.to_string());
        println!(
            "{}Switched to model: {}{}{}",
            crate::BOLD,
            crate::BLUE,
            name,
            crate::RESET
        );
        Ok(())
    } else {
        // Still switch even if not in list (model might be pullable)
        provider.select_model(name.to_string());
        println!(
            "{}Set model to: {}{}{}",
            crate::BOLD,
            crate::BLUE,
            name,
            crate::RESET
        );
        Ok(())
    }
}
