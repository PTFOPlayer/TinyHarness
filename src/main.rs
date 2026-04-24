pub mod provider;
pub mod system_prompt;
pub mod tools;

use crate::{
    system_prompt::system_prompt,
    tools::{ToolManager, ls::ls_tool_entry, read::read_tool_entry},
};
use std::{
    error::Error,
    io::{self, Write},
    sync::Arc,
};

use ollama_rs::generation::{chat::ChatMessageResponse, tools::ToolCall};
use tokio::sync::{Mutex, mpsc};

use crate::provider::{Message, Provider, Role, ollama::OllamaProvider};

// ANSI color codes
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const BLUE: &str = "\x1b[34m";
const ORANGE: &str = "\x1b[38;5;208m";

#[derive(clap::Parser, Debug)]
struct Args {
    #[arg(short, long)]
    ollama: bool,
    #[arg(short, long)]
    llamacpp: bool,
    #[arg(short, long)]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let provider: Arc<Mutex<dyn Provider + Send + Sync>> = Arc::new(Mutex::new(
        OllamaProvider::new(String::from("http://127.0.0.1:11434")),
    ));

    {
        let mut provider = provider.lock().await;
        provider.select_model(String::from("gemma4:31b-cloud"));
    }

    let (send, mut recv) = mpsc::channel::<ChatMessageResponse>(1024);

    let mut tool_manager = ToolManager::new();
    tool_manager.register_tool(ls_tool_entry());
    tool_manager.register_tool(read_tool_entry());

    let ollama_tools = tool_manager.get_ollama_tools();
    let mut messages = vec![system_prompt()];

    let mut stdout = io::stdout();
    stdout.write("╔════════════════════════════════════════════════════════╗\n".as_bytes())?;
    stdout.write("║           TinyHarness AI Assistant                     ║\n".as_bytes())?;
    stdout.write("╚════════════════════════════════════════════════════════╝\n\n".as_bytes())?;
    stdout.flush()?;

    loop {
        stdout.write_all(format!("{}> {}{}", BOLD, RESET, BLUE).as_bytes())?;
        stdout.flush()?;

        let mut user_input = String::new();
        io::stdin()
            .read_line(&mut user_input)
            .expect("Failed to read line");
        user_input = user_input.trim().to_string();

        if user_input.is_empty() {
            continue;
        }

        messages.push(Message {
            role: Role::User,
            content: user_input.clone(),
            tool_calls: vec![],
        });

        // Drain any leftover messages in the channel
        while let Ok(_) = recv.try_recv() {}

        // Main conversation loop with tool support
        loop {
            let messages_cloned = messages.clone();
            let send_cloned = send.clone();
            let provider_cloned = Arc::clone(&provider);
            let tools = ollama_tools.clone();
            let cloned_user_input = user_input.clone();
            tokio::spawn(async move {
                let mut provider = provider_cloned.lock().await;
                provider
                    .chat(messages_cloned, cloned_user_input, send_cloned, tools)
                    .await;
            });

            let mut response_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut received_done = false;

            stdout.write(ORANGE.as_bytes())?;

            while let Some(msg) = recv.recv().await {
                if !msg.message.tool_calls.is_empty() {
                    tool_calls = msg.message.tool_calls.clone();
                }
                
                if msg.done {
                    received_done = true;
                }

                if !msg.message.content.is_empty() {
                    response_content.push_str(&msg.message.content);
                    stdout.write(format!("{}", msg.message.content).as_bytes())?;
                    stdout.flush()?;
                }
                
                if received_done {
                    break;
                }
            }

            stdout.write(RESET.as_bytes())?;

            if !tool_calls.is_empty() {
                stdout.write(format!("\n{}Tool call(s) detected: {}{}\n\n", BOLD, tool_calls.len(), RESET).as_bytes())?;
                
                messages.push(Message {
                    role: Role::Assistant,
                    content: response_content.clone(),
                    tool_calls: tool_calls.clone(),
                });

                for call in &tool_calls {
                    stdout.write(format!("  Executing tool: {}\n", call.function.name).as_bytes())?;
                    stdout.flush()?;
                    let result = tool_manager
                        .execute_tool_call(&call.function.name, &call.function.arguments);
                    stdout.write(format!("  Result: {}\n", result.lines().next().unwrap_or(&result)).as_bytes())?;
                    stdout.flush()?;
                    messages.push(Message {
                        role: Role::System,
                        content: format!(
                            "Tool '{}' result:\n{}\n\nUse this result to continue helping the user.",
                            call.function.name, result
                        ),
                        tool_calls: vec![],
                    });
                }

                continue;
            }

            messages.push(Message {
                role: Role::Assistant,
                content: response_content,
                tool_calls: vec![],
            });
            break;
        }

        stdout.write("\n".as_bytes())?;
        stdout.flush()?;
    }
}
