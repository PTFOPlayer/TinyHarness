pub mod apikey;
pub mod audit;
pub mod clear;
pub mod command;
pub mod compact;
pub mod config_settings;
pub mod context;
pub mod exit;
pub mod files;
pub mod help;
pub mod init;
pub mod mode;
pub mod models;
pub mod registry;
pub mod sessions;
pub mod settings;
pub mod skill;

use std::sync::Arc;

use tinyharness_lib::{context::WorkspaceContext, provider::Provider};

use tokio::sync::Mutex;

use crate::commands::{
    compact::CompactCommand,
    config_settings::{RetriesCommand, ThinkCommand, TimeoutCommand},
    init::InitCommand,
    models::ModelsCommand,
};

pub use files::FileContext;
pub use init::InitResult;
pub use registry::{CommandContext, CommandRegistry, CommandResult};

/// Require a non-empty argument. Returns `Err(usage_message)` if the argument
/// is missing or empty, standardizing error messages across commands.
pub fn require_arg<'a>(arg: Option<&'a str>, usage: &str) -> Result<&'a str, String> {
    match arg {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(format!("Usage: {}", usage)),
    }
}

/// Build the default command registry with all built-in commands.
pub fn build_registry() -> CommandRegistry {
    let mut reg = CommandRegistry::new();

    // ── Sync commands (simple closures, no async needed) ──────────────────

    reg.register_sync("/clear", "Clear the terminal screen", |_arg, _ctx, _msg| {
        crate::commands::clear::execute();
        Ok(CommandResult::Ok)
    });

    reg.register_sync("/exit", "Exit the application", |_arg, ctx, _msg| {
        crate::commands::exit::execute();
        ctx.exit_requested = true;
        Ok(CommandResult::Ok)
    });

    reg.register_sync(
        "/context",
        "Show the workspace context available to the agent",
        |_arg, ctx, _msg| {
            crate::commands::context::execute(&ctx.workspace_ctx);
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync_with_usage(
        "/rename",
        "Rename the current session",
        "/rename <name>",
        |arg, _ctx, _msg| {
            let name = require_arg(
                arg,
                "/rename <name> — give the current session a descriptive name",
            )?;
            Ok(CommandResult::RenameSession(name.to_string()))
        },
    );

    // ── Mode ──────────────────────────────────────────────────────────────

    reg.register_sync_with_usage(
        "/mode",
        "Show or switch mode (casual/planning/agent/research)",
        "/mode [mode]",
        |arg, ctx, msg| crate::commands::mode::execute(arg, ctx, msg),
    );

    // ── Settings ───────────────────────────────────────────────────────────

    reg.register_sync_with_usage(
        "/settings",
        "Show current settings. Use 'all' to list all safe commands.",
        "/settings [all]",
        |arg, _ctx, _msg| crate::commands::settings::execute(arg),
    );

    reg.register_sync_with_usage(
        "/apikey",
        "Set or show the Ollama API key for web search. Use /apikey clear to remove it.",
        "/apikey [key]",
        |arg, _ctx, _msg| {
            let a = arg.unwrap_or("");
            if a.is_empty() {
                crate::commands::apikey::execute_show();
            } else if a == "clear" {
                crate::commands::apikey::execute_clear();
            } else {
                crate::commands::apikey::execute_set(a);
            }
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync_with_usage(
        "/contextlimit",
        "Show or set the context limit for warning calculations (default: model default)",
        "/contextlimit [tokens]",
        |arg, _ctx, _msg| crate::commands::config_settings::execute_context_limit(arg),
    );

    reg.register_sync_with_usage(
        "/autoaccept",
        "Show or toggle auto-accept for safe read-only commands (default: on)",
        "/autoaccept [on|off]",
        |arg, _ctx, _msg| crate::commands::config_settings::execute_autoaccept(arg),
    );

    // ── Command management ────────────────────────────────────────────────

    reg.register_sync_with_usage(
        "/command",
        "Manage auto-accepted and denied commands",
        "/command [list|add|rm|deny|undeny|reset|resetdeny|help]",
        |arg, _ctx, _msg| crate::commands::command::execute(arg.unwrap_or("")),
    );

    // ── Audit ──────────────────────────────────────────────────────────────

    reg.register_sync_with_usage(
        "/audit",
        "View command execution audit log",
        "/audit [last|session|clear]",
        |arg, _ctx, _msg| {
            crate::commands::audit::execute(arg.unwrap_or(""));
            Ok(CommandResult::Ok)
        },
    );

    // ── Sessions ──────────────────────────────────────────────────────────

    reg.register_sync("/sessions", "List all saved sessions", |_arg, ctx, _msg| {
        crate::commands::sessions::execute_list(ctx.session_id.as_deref());
        Ok(CommandResult::Ok)
    });

    reg.register_sync_with_usage(
        "/session",
        "Switch to an existing session (accepts ID prefix)",
        "/session <id>",
        |arg, ctx, _msg| {
            let a = require_arg(
                arg,
                "/session <id> — use /sessions to list available sessions",
            )?;
            if a.starts_with("delete ") || a == "delete" {
                let id = a.strip_prefix("delete ").unwrap_or("").trim();
                let id = require_arg(
                    if id.is_empty() { None } else { Some(id) },
                    "/session delete <id|name> — use /sessions to list available sessions",
                )?;
                crate::commands::sessions::execute_delete(id, ctx.session_id.as_deref());
                return Ok(CommandResult::Ok);
            }
            Ok(CommandResult::SwitchSession(a.to_string()))
        },
    );

    // ── File pinning ──────────────────────────────────────────────────────

    reg.register_sync_with_usage(
        "/add",
        "Pin a file into the AI's context so it's always available",
        "/add <path>",
        |arg, ctx, msg| {
            let path = require_arg(arg, "/add <file_path> — e.g. /add src/main.rs")?;
            crate::commands::files::execute_add(&mut ctx.file_context, path);
            ctx.refresh_system_prompt(msg);
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync_with_usage(
        "/drop",
        "Remove a pinned file from context",
        "/drop <path>",
        |arg, ctx, msg| {
            let path = require_arg(arg, "/drop <file_path> — e.g. /drop src/main.rs")?;
            crate::commands::files::execute_drop(&mut ctx.file_context, path);
            ctx.refresh_system_prompt(msg);
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync(
        "/files",
        "List all pinned files in context",
        |_arg, ctx, _msg| {
            crate::commands::files::execute_list(&ctx.file_context);
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync(
        "/dropall",
        "Remove all pinned files from context",
        |_arg, ctx, msg| {
            crate::commands::files::execute_clear(&mut ctx.file_context);
            ctx.refresh_system_prompt(msg);
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync(
        "/refresh",
        "Re-read all pinned files from disk (updates content)",
        |_arg, ctx, msg| {
            crate::commands::files::execute_refresh(&mut ctx.file_context);
            ctx.refresh_system_prompt(msg);
            Ok(CommandResult::Ok)
        },
    );

    // ── Skills ────────────────────────────────────────────────────────────

    reg.register_sync("/skills", "List all available skills", |_arg, ctx, _msg| {
        crate::commands::skill::execute_list(&ctx.skill_registry, &ctx.active_skills);
        Ok(CommandResult::Ok)
    });

    reg.register_sync_with_usage(
        "/skill",
        "Show details and content of a skill",
        "/skill <name>",
        |arg, ctx, _msg| {
            let name = arg.unwrap_or("").to_string();
            if name.is_empty() {
                crate::commands::skill::execute_list(&ctx.skill_registry, &ctx.active_skills);
                return Ok(CommandResult::Ok);
            }
            // Check for "use <name>" subcommand
            if let Some(skill_name) = name.strip_prefix("use ") {
                let skill_name = skill_name.trim().to_string();
                if skill_name.is_empty() {
                    crate::commands::skill::execute_list(&ctx.skill_registry, &ctx.active_skills);
                    return Ok(CommandResult::Ok);
                }
                return crate::commands::skill::handle_skill_use(&skill_name, ctx);
            }
            let mut stdout = std::io::stdout();
            crate::commands::skill::execute_show(
                &ctx.skill_registry,
                &name,
                &ctx.active_skills,
                &mut stdout,
            );
            Ok(CommandResult::Ok)
        },
    );

    reg.register_sync_with_usage(
        "/use",
        "Activate a skill, injecting its instructions into the conversation",
        "/use <name>",
        |arg, ctx, _msg| {
            let name = require_arg(arg, "/use <name>")?;
            crate::commands::skill::handle_skill_use(name, ctx)
        },
    );

    reg.register_sync_with_usage(
        "/unload",
        "Deactivate a previously loaded skill",
        "/unload <name>",
        |arg, ctx, _msg| {
            let name = require_arg(arg, "/unload <name>")?;
            if ctx
                .active_skills
                .iter()
                .any(|s| s.eq_ignore_ascii_case(name))
            {
                Ok(CommandResult::SkillUnload(name.to_string()))
            } else {
                println!(
                    "{}Skill '{}' is not currently active.{}",
                    crate::style::ORANGE,
                    name,
                    crate::style::RESET
                );
                if !ctx.active_skills.is_empty() {
                    let active = ctx.active_skills.join(", ");
                    println!(
                        "{}Active skills: {}{}{}",
                        crate::style::GRAY,
                        crate::style::CYAN,
                        active,
                        crate::style::RESET
                    );
                }
                Ok(CommandResult::Ok)
            }
        },
    );

    // ── Async commands (need provider.lock().await) ────────────────────────

    reg.register(ModelsCommand);
    reg.register_alias(
        "/model",
        "/models",
        None,
        "Switch to a different model (alias for /models)",
    );
    reg.register(CompactCommand);
    reg.register(InitCommand);
    reg.register(TimeoutCommand);
    reg.register(RetriesCommand);
    reg.register(ThinkCommand);

    // ── Aliases ───────────────────────────────────────────────────────────

    // Mode aliases: /plan, /agent, /research, /casual → /mode <mode>
    reg.register_alias(
        "/plan",
        "/mode",
        Some("planning"),
        "Switch to planning mode (alias for /mode planning)",
    );
    reg.register_alias(
        "/agent",
        "/mode",
        Some("agent"),
        "Switch to agent mode (alias for /mode agent)",
    );
    reg.register_alias(
        "/research",
        "/mode",
        Some("research"),
        "Switch to research mode (alias for /mode research)",
    );
    reg.register_alias(
        "/casual",
        "/mode",
        Some("casual"),
        "Switch to casual mode (alias for /mode casual)",
    );

    // Exit alias: /quit → /exit
    reg.register_alias("/quit", "/exit", None, "Exit the application");

    // ── Help (registered last, after descriptions are frozen) ─────────────

    reg.freeze_descriptions();
    let descs = reg.descriptions().to_vec();
    reg.register_sync(
        "/help",
        "Show this help message",
        move |_arg, _ctx, _msg| {
            crate::commands::help::execute(&descs);
            Ok(CommandResult::Ok)
        },
    );

    reg
}

/// Create a new CommandContext with the given provider and workspace context.
pub fn create_context(
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    workspace_ctx: WorkspaceContext,
    prompts_dir: std::path::PathBuf,
) -> CommandContext {
    CommandContext::new(provider, workspace_ctx, prompts_dir)
}
