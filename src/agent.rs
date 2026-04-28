use std::{
    error::Error,
    io::{self, Write},
    sync::Arc,
};

use rustyline::{
    Completer, Editor, Helper, Highlighter, Hinter, completion::Completer, error::ReadlineError,
    highlight::Highlighter, hint::Hinter, validate::Validator,
};

use tokio::sync::{Mutex, mpsc};

use crate::style::*;
use crate::{
    commands::CommandDispatcher,
    mode::AgentMode,
    provider::{ChatMessageResponse, Message, Provider, Role, ToolCall, ToolInfo},
    tools::ToolManager,
};

#[derive(Completer, Helper, Highlighter, Hinter)]
struct CommandHelper {
    #[rustyline(Completer)]
    completer: CommandCompleter,
    #[rustyline(Hinter)]
    hinter: CommandHinter,
    #[rustyline(Highlighter)]
    highlighter: CommandHighlighter,
}

impl Validator for CommandHelper {}

struct CommandCompleter;

impl Completer for CommandCompleter {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        if !line.starts_with('/') || pos == 0 {
            return Ok((0, vec![]));
        }

        let prefix = &line[..pos];
        let cmd_prefix = prefix.to_lowercase();

        let matches: Vec<String> = CommandDispatcher::command_names()
            .iter()
            .filter(|name| name.starts_with(&cmd_prefix))
            .take(3)
            .map(|s| s.to_string())
            .collect();

        if matches.is_empty() {
            return Ok((0, vec![]));
        }

        Ok((0, matches))
    }
}

struct CommandHinter;

impl Hinter for CommandHinter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if !line.starts_with('/') || pos == 0 || line.len() != pos {
            return None;
        }

        let prefix = line.to_lowercase();
        let matches: Vec<&str> = CommandDispatcher::command_names()
            .iter()
            .filter(|name| name.starts_with(&prefix))
            .take(3)
            .copied()
            .collect();

        if matches.is_empty() {
            return None;
        }

        if matches.len() == 1 {
            let hint = matches[0][pos..].to_string();
            if !hint.is_empty() {
                return Some(hint);
            }
        }

        let suggestions = matches.join("  ");
        Some(format!("  ({})", suggestions))
    }
}

struct CommandHighlighter;

impl Highlighter for CommandHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        if line.starts_with('/') {
            std::borrow::Cow::Owned(format!("{}{}{}", BLUE, line, RESET))
        } else {
            std::borrow::Cow::Borrowed(line)
        }
    }

    fn highlight_hint<'l>(&self, hint: &'l str) -> std::borrow::Cow<'l, str> {
        std::borrow::Cow::Owned(format!("{}{}{}", GRAY, hint, RESET))
    }

    fn highlight_candidate<'l>(
        &self,
        candidate: &'l str,
        _completion: rustyline::CompletionType,
    ) -> std::borrow::Cow<'l, str> {
        std::borrow::Cow::Owned(format!("{}{}{}", BLUE, candidate, RESET))
    }
}

pub async fn run_agent_loop(
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    tool_manager: ToolManager,
    ollama_tools: Vec<ToolInfo>,
    messages: &mut Vec<Message>,
    dispatcher: &mut CommandDispatcher,
) -> Result<(), Box<dyn Error>> {
    let (send, mut recv) = mpsc::channel::<ChatMessageResponse>(1024);

    let mut stdout = io::stdout();
    stdout.write("╔════════════════════════════════════════════════════════╗\n".as_bytes())?;
    stdout.write("║           TinyHarness AI Assistant                     ║\n".as_bytes())?;
    stdout.write("╚════════════════════════════════════════════════════════╝\n\n".as_bytes())?;
    stdout.write(
        format!(
            "{}Tip:{} Type {} to see available commands\n\n",
            GRAY, RESET, "/help"
        )
        .as_bytes(),
    )?;
    stdout.flush()?;

    let helper = CommandHelper {
        completer: CommandCompleter,
        hinter: CommandHinter,
        highlighter: CommandHighlighter,
    };
    let history_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/tinyharness"))
        .unwrap_or_else(|_| std::path::PathBuf::from(".tinyharness_history"));
    std::fs::create_dir_all(&history_dir).ok();
    let history_path = history_dir.join("history.txt");
    let mut rl = Editor::new()?;
    rl.set_helper(Some(helper));
    rl.load_history(&history_path).ok();

    loop {
        let mode_label = dispatcher.current_mode.to_string();
        let prompt = format!("{}[{}]{} > {}{}", BOLD, mode_label, RESET, BLUE, RESET);
        let readline = rl.readline(&prompt);
        let user_input = match readline {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                rl.add_history_entry(&trimmed)?;
                trimmed
            }
            Err(ReadlineError::Interrupted) => {
                stdout.write("\n".as_bytes())?;
                stdout.write(
                    format!(
                        "{}Use {}/exit{} or {}{}Ctrl+D{} to exit.\n",
                        GRAY, BLUE, GRAY, GRAY, BOLD, RESET
                    )
                    .as_bytes(),
                )?;
                stdout.flush()?;
                continue;
            }
            Err(ReadlineError::Eof) => {
                stdout.write("\n".as_bytes())?;
                break;
            }
            Err(err) => {
                eprintln!("{}Error reading input: {}{}", RED, err, RESET);
                break;
            }
        };

        if user_input.starts_with('/') {
            match CommandDispatcher::parse(&user_input) {
                Some(cmd) => {
                    if let Err(e) = dispatcher.dispatch(cmd, messages).await {
                        eprintln!("{}{}{}", RED, e, RESET);
                    }
                    if dispatcher.exit_requested {
                        break;
                    }
                }
                None => {
                    eprintln!(
                        "{}Unknown command: {}{}{}\n  Type {}/help{} for available commands.{}",
                        RED, BLUE, user_input, RED, BLUE, RED, RESET
                    );
                }
            }
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
            // Filter tools based on current mode
            let tools = match dispatcher.current_mode {
                AgentMode::Agent => ollama_tools.clone(),
                AgentMode::Planning => tool_manager.get_readonly_tools(),
                AgentMode::Casual => Vec::new(),
            };
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

            if !received_done {
                stdout.write(RESET.as_bytes())?;
                stdout.write(
                    format!(
                        "{}Error:{} Provider request failed or was interrupted.\n",
                        RED, RESET
                    )
                    .as_bytes(),
                )?;
                messages.pop();
                break;
            }

            stdout.write(RESET.as_bytes())?;

            if handle_tool_calls(
                &tool_calls,
                &response_content,
                messages,
                &tool_manager,
                &mut stdout,
            )
            .await?
            {
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

    // Save history on exit
    rl.save_history(&history_path).ok();

    Ok(())
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

    stdout.write(
        format!(
            "\n{}Tool call(s) detected: {}{}\n\n",
            BOLD,
            tool_calls.len(),
            RESET
        )
        .as_bytes(),
    )?;

    messages.push(Message {
        role: Role::Assistant,
        content: response_content.to_string(),
        tool_calls: tool_calls.to_vec(),
    });

    let sensitive_tools = ["run", "write", "edit"];

    for call in tool_calls {
        let needs_confirmation = sensitive_tools.contains(&call.function.name.as_str());

        if needs_confirmation {
            stdout.write(
                format!(
                    "\n{}{}⚠ Tool '{}' requires confirmation{}",
                    RED, BOLD, call.function.name, RESET
                )
                .as_bytes(),
            )?;

            // Pretty-print arguments in a cleaner format
            let args_str = match &call.function.arguments {
                serde_json::Value::Object(map) => {
                    let mut lines: Vec<String> = Vec::new();
                    for (key, val) in map {
                        let val_str = match val {
                            serde_json::Value::String(s) => {
                                if s.len() > 80 {
                                    format!("{}... ({} chars)", &s[..77], s.len())
                                } else {
                                    s.clone()
                                }
                            }
                            other => other.to_string(),
                        };
                        lines.push(format!("    {}: {}", key, val_str));
                    }
                    lines.join("\n")
                }
                other => format!("  {}", serde_json::to_string_pretty(other).unwrap_or_default()),
            };
            stdout.write(format!("{}\n", args_str).as_bytes())?;

            stdout.write(format!("{}Proceed? (y/N):{} ", BOLD, RESET).as_bytes())?;
            stdout.flush()?;

            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .expect("Failed to read line");
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                stdout
                    .write(format!("{}  Skipped by user{}{}\n", ORANGE, RESET, BOLD).as_bytes())?;
                stdout.flush()?;

                // Compact argument summary for the system message
                let args_summary = format_args_summary(&call.function.arguments);
                messages.push(Message {
                    role: Role::System,
                    content: format!(
                        "The user denied the '{}' tool call with arguments: {}\n\nTell the user you cannot proceed with that action unless they approve it.",
                        call.function.name, args_summary
                    ),
                    tool_calls: vec![],
                });
                continue;
            }
        }

        stdout.write(format!("  Executing tool: {}\n", call.function.name).as_bytes())?;
        stdout.flush()?;
        let result = tool_manager.execute_tool_call(&call.function.name, &call.function.arguments);
        stdout.write(
            format!("  Result: {}\n", result.lines().next().unwrap_or(&result)).as_bytes(),
        )?;
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

/// Format tool call arguments as a compact single-line summary.
fn format_args_summary(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, val)| {
                    let val_str = match val {
                        serde_json::Value::String(s) => {
                            if s.len() > 60 {
                                format!("\"{}...\"", &s[..57])
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        other => other.to_string(),
                    };
                    format!("{}={}", key, val_str)
                })
                .collect();
            parts.join(", ")
        }
        other => other.to_string(),
    }
}
