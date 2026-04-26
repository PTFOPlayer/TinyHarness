pub mod provider;
pub mod system_prompt;
pub mod tools;

use crate::{
    system_prompt::system_prompt,
    tools::{
        ToolManager, edit::edit_tool_entry, glob::glob_tool_entry, grep::grep_tool_entry,
        ls::ls_tool_entry, read::read_tool_entry, run::run_tool_entry, write::write_tool_entry,
    },
};
use std::{
    error::Error,
    io::{self, Write},
    sync::Arc,
};

use clap::Parser;
use tokio::sync::{Mutex, mpsc};

use crate::provider::{
    ChatMessageResponse, Message, Provider, Role, ToolCall,
    llama_cpp::LlamaCppProvider, ollama::OllamaProvider,
};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const BLUE: &str = "\x1b[34m";
const ORANGE: &str = "\x1b[38;5;208m";

#[derive(clap::Parser, Debug)]
struct Args {
    #[arg(short, long)]
    ollama: bool,
    #[arg(short, long)]
    llama_cpp: bool,
    #[arg(short, long, default_value_t=String::new())]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let default_url = if args.llama_cpp {
        "http://127.0.0.1:8080"
    } else {
        "http://127.0.0.1:11434"
    };

    let url = if args.url.is_empty() {
        default_url.to_string()
    } else {
        args.url
    };

    let provider: Arc<Mutex<dyn Provider + Send + Sync>> = if args.llama_cpp {
        let llama = LlamaCppProvider::new(url);
        if let Err(e) = llama.health_check().await {
            eprintln!("{}Error:{} LlamaCpp health check failed: {}", BOLD, RESET, e);
            std::process::exit(1);
        }
        Arc::new(Mutex::new(llama))
    } else {
        Arc::new(Mutex::new(OllamaProvider::new(url)))
    };

    {
        let mut provider = provider.lock().await;
        provider.select_model(String::from("gemma4:31b-cloud"));
    }

    let (send, mut recv) = mpsc::channel::<ChatMessageResponse>(1024);

    let mut tool_manager = ToolManager::new();
    tool_manager.register_tool(ls_tool_entry());
    tool_manager.register_tool(read_tool_entry());
    tool_manager.register_tool(write_tool_entry());
    tool_manager.register_tool(edit_tool_entry());
    tool_manager.register_tool(grep_tool_entry());
    tool_manager.register_tool(run_tool_entry());
    tool_manager.register_tool(glob_tool_entry());

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

            if handle_tool_calls(&tool_calls, &response_content, &mut messages, &tool_manager, &mut stdout).await? {
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

async fn handle_tool_calls<W: Write>(
    tool_calls: &[ToolCall],
    response_content: &str,
    messages: &mut Vec<Message>,
    tool_manager: &ToolManager,
    stdout: &mut W,
) -> Result<bool, Box<dyn Error>> {
    if tool_calls.is_empty() {
        return Ok(false);
    }

    stdout.write(format!("\n{}Tool call(s) detected: {}{}\n\n", BOLD, tool_calls.len(), RESET).as_bytes())?;

    messages.push(Message {
        role: Role::Assistant,
        content: response_content.to_string(),
        tool_calls: tool_calls.to_vec(),
    });

    let sensitive_tools = ["run", "write", "edit"];

    for call in tool_calls {
        let needs_confirmation = sensitive_tools.contains(&call.function.name.as_str());

        if needs_confirmation {
            stdout.write(format!(
                "{}{}⚠ Tool '{}' requires confirmation{}",
                RED, BOLD, call.function.name, RESET
            ).as_bytes())?;
            stdout.write(format!(
                "\n  Arguments: {}\n",
                serde_json::to_string_pretty(&call.function.arguments).unwrap_or_default()
            ).as_bytes())?;
            stdout.write(format!("{}Proceed? (y/N):{} ", BOLD, RESET).as_bytes())?;
            stdout.flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read line");
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                stdout.write(format!("{}  Skipped by user{}{}\n", ORANGE, RESET, BOLD).as_bytes())?;
                stdout.flush()?;
                messages.push(Message {
                    role: Role::System,
                    content: format!(
                        "The user denied the '{}' tool call with arguments: {}\n\nTell the user you cannot proceed with that action unless they approve it.",
                        call.function.name,
                        serde_json::to_string_pretty(&call.function.arguments).unwrap_or_default()
                    ),
                    tool_calls: vec![],
                });
                continue;
            }
        }

        stdout.write(format!("  Executing tool: {}\n", call.function.name).as_bytes())?;
        stdout.flush()?;
        let result = tool_manager.execute_tool_call(&call.function.name, &call.function.arguments);
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

    Ok(true)
}
