use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use tokio::sync::Mutex;

use tinyharness_lib::{
    config::{load_settings, save_settings},
    context::WorkspaceContext,
    mode::AgentMode,
    provider::{Message, Provider, Role},
    skill::SkillRegistry,
};

use crate::commands::files::FileContext;
use crate::commands::init::InitResult;

// ── CommandResult ────────────────────────────────────────────────────────────

/// Result of dispatching a command.
pub enum CommandResult {
    /// Command completed normally.
    Ok,
    /// The user wants to switch to a different session.
    SwitchSession(String),
    /// The user wants to rename the current session.
    RenameSession(String),
    /// The /init command was run — workspace context should be refreshed.
    Init(InitResult),
    /// The user wants to activate a skill, injecting its instructions into the conversation.
    SkillUse(String),
    /// The user wants to deactivate (unload) a skill.
    SkillUnload(String),
}

// ── CommandContext ────────────────────────────────────────────────────────────

/// Context passed to every command handler.
/// Holds the shared state that commands may need to read or mutate.
pub struct CommandContext {
    pub provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    pub exit_requested: bool,
    pub current_mode: AgentMode,
    pub workspace_ctx: WorkspaceContext,
    pub file_context: FileContext,
    pub session_id: Option<String>,
    pub skill_registry: SkillRegistry,
    /// Names of currently active (loaded) skills.
    pub active_skills: Vec<String>,
    /// Cached command descriptions from the registry (for /help).
    pub command_descriptions: Vec<(&'static str, &'static str)>,
}

impl CommandContext {
    pub fn new(
        provider: Arc<Mutex<dyn Provider + Send + Sync>>,
        workspace_ctx: WorkspaceContext,
    ) -> Self {
        CommandContext {
            provider,
            exit_requested: false,
            current_mode: AgentMode::Casual,
            workspace_ctx,
            file_context: FileContext::new(),
            session_id: None,
            skill_registry: SkillRegistry::discover(),
            active_skills: Vec::new(),
            command_descriptions: Vec::new(),
        }
    }

    /// Update the system prompt message in the conversation to reflect the current
    /// mode, workspace context, and pinned files. Call this after any change that
    /// affects the system prompt content (mode switch, add/drop/refresh files, etc.).
    pub fn refresh_system_prompt(&self, messages: &mut [Message]) {
        if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == Role::System) {
            sys_msg.content = self.build_system_prompt();
        }
    }

    /// Switch the current mode to `new_mode`. Updates the system prompt in the
    /// conversation and auto-saves the new mode to settings.
    /// Returns `Ok(())` on success or an error string if the mode is unchanged/invalid.
    pub fn switch_mode(
        &mut self,
        new_mode: AgentMode,
        messages: &mut [Message],
    ) -> Result<(), String> {
        if new_mode == self.current_mode {
            return Err(format!("Already in '{}' mode", new_mode));
        }

        self.current_mode = new_mode;
        self.refresh_system_prompt(messages);

        // Auto-save mode
        let mut settings = load_settings();
        settings.preferred_mode = self.current_mode;
        save_settings(&settings);

        Ok(())
    }

    /// Build the system prompt for the current mode, appending workspace context,
    /// pinned file content, skill index, and active skill instructions.
    pub fn build_system_prompt(&self) -> String {
        let mut prompt = format!(
            "{}\n\n---\n{}",
            self.current_mode.system_prompt(),
            self.workspace_ctx.format()
        );

        // Inject pinned file content
        if !self.file_context.is_empty() {
            prompt.push_str(&self.file_context.format_for_prompt());
        }

        // Inject skill index for model auto-invocation
        let skill_index = self.skill_registry.format_index_for_prompt();
        if !skill_index.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&skill_index);
        }

        // Inject active skill instructions
        for name in &self.active_skills {
            if let Some(skill) = self.skill_registry.get(name) {
                prompt.push_str("\n\n");
                prompt.push_str(&self.skill_registry.format_skill_content(skill));
            }
        }

        prompt
    }
}

// ── Command trait ────────────────────────────────────────────────────────────

/// A self-contained command definition.
///
/// Each command implements this trait to provide its name, aliases, description,
/// usage string, and execution logic. The registry dispatches user input to the
/// appropriate `Command` implementation.
pub trait Command: Send + Sync {
    /// Primary name (e.g., "/help").
    fn name(&self) -> &'static str;

    /// Aliases that also invoke this command (e.g., `["/quit"]` for exit).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    /// One-line description for /help.
    fn description(&self) -> &'static str;

    /// Usage string (e.g., "/model <name>"). Defaults to the command name.
    fn usage(&self) -> &'static str {
        self.name()
    }

    /// Parse and execute the command.
    ///
    /// `raw_arg` is the argument portion of the input (after the command name),
    /// or `None` if no argument was provided.
    ///
    /// Returns a `CommandResult` on success, or an error string on failure.
    fn execute<'a>(
        &'a self,
        raw_arg: Option<&str>,
        ctx: &'a mut CommandContext,
        messages: &'a mut Vec<Message>,
    ) -> Pin<Box<dyn Future<Output = Result<CommandResult, String>> + Send + 'a>>;
}

// ── SyncCommand ──────────────────────────────────────────────────────────────

/// A synchronous command that wraps a simple function.
///
/// This eliminates the need for the full `Command` trait boilerplate when a
/// command's execute logic is purely synchronous (no `.await`, no provider lock).
/// The closure receives `(arg, ctx, messages)` and returns `Result<CommandResult, String>`.
pub struct SyncCommand<F>
where
    F: Fn(Option<&str>, &mut CommandContext, &mut Vec<Message>) -> Result<CommandResult, String>
        + Send
        + Sync,
{
    pub name_str: &'static str,
    pub description_str: &'static str,
    pub usage_str: &'static str,
    pub aliases_str: &'static [&'static str],
    pub handler: F,
}

impl<F> Command for SyncCommand<F>
where
    F: Fn(Option<&str>, &mut CommandContext, &mut Vec<Message>) -> Result<CommandResult, String>
        + Send
        + Sync,
{
    fn name(&self) -> &'static str {
        self.name_str
    }

    fn aliases(&self) -> &'static [&'static str] {
        self.aliases_str
    }

    fn description(&self) -> &'static str {
        self.description_str
    }

    fn usage(&self) -> &'static str {
        self.usage_str
    }

    fn execute<'a>(
        &'a self,
        raw_arg: Option<&str>,
        ctx: &'a mut CommandContext,
        messages: &'a mut Vec<Message>,
    ) -> Pin<Box<dyn Future<Output = Result<CommandResult, String>> + Send + 'a>> {
        // Call the sync handler and wrap the result in an immediately-resolved future
        match (self.handler)(raw_arg, ctx, messages) {
            Ok(result) => Box::pin(async move { Ok(result) }),
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }
}

// ── AliasCommand ─────────────────────────────────────────────────────────────

/// A lightweight alias that delegates to another command with an optional fixed argument.
///
/// For example, `/plan` is an alias for `/mode` with the fixed arg `"planning"`.
pub struct AliasCommand {
    pub alias_name: &'static str,
    pub target_name: &'static str,
    /// If set, this arg is passed to the target instead of the user's arg.
    pub fixed_arg: Option<&'static str>,
    pub description: &'static str,
}

impl Command for AliasCommand {
    fn name(&self) -> &'static str {
        self.alias_name
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn usage(&self) -> &'static str {
        self.alias_name
    }

    fn execute<'a>(
        &self,
        raw_arg: Option<&str>,
        _ctx: &'a mut CommandContext,
        _messages: &'a mut Vec<Message>,
    ) -> Pin<Box<dyn Future<Output = Result<CommandResult, String>> + Send + 'a>> {
        // Aliases are resolved by the registry's dispatch method.
        // This method should never be called directly.
        let _ = raw_arg;
        Box::pin(async { Ok(CommandResult::Ok) })
    }
}

// ── CommandRegistry ──────────────────────────────────────────────────────────

/// The command registry — maps command names and aliases to their handler implementations.
pub struct CommandRegistry {
    /// Primary command handlers, keyed by primary name.
    commands: HashMap<&'static str, Box<dyn Command>>,
    /// Alias → target command name mapping.
    aliases: HashMap<&'static str, &'static str>,
    /// Fixed args for aliases (alias → arg to pass instead of user input).
    alias_fixed_args: HashMap<&'static str, &'static str>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        CommandRegistry {
            commands: HashMap::new(),
            aliases: HashMap::new(),
            alias_fixed_args: HashMap::new(),
        }
    }

    /// Register a command implementation.
    /// Also registers any aliases the command declares.
    pub fn register(&mut self, cmd: impl Command + 'static) {
        let name = cmd.name();
        let aliases = cmd.aliases();
        self.commands.insert(name, Box::new(cmd));

        // Register aliases that point to the primary name
        for alias in aliases {
            self.aliases.insert(alias, name);
        }
    }

    /// Register an alias for an existing command.
    ///
    /// If `fixed_arg` is provided, that string is passed as the argument
    /// instead of whatever the user typed.
    pub fn register_alias(
        &mut self,
        alias: &'static str,
        target: &'static str,
        fixed_arg: Option<&'static str>,
        description: &'static str,
    ) {
        self.aliases.insert(alias, target);
        if let Some(arg) = fixed_arg {
            self.alias_fixed_args.insert(alias, arg);
        }
        // Store the description so it appears in /help
        let _ = description; // Used by command_descriptions()
    }

    /// Register a synchronous command using a closure.
    ///
    /// This is a convenience method that creates a `SyncCommand` internally,
    /// avoiding the need for a full `impl Command` block.
    ///
    /// # Example
    /// ```ignore
    /// reg.register_sync("/clear", "Clear the terminal screen",
    ///     |_arg, _ctx, _msg| { print!("{}", CLEAR_SCREEN); Ok(CommandResult::Ok) });
    /// ```
    pub fn register_sync<F>(&mut self, name: &'static str, description: &'static str, handler: F)
    where
        F: Fn(
                Option<&str>,
                &mut CommandContext,
                &mut Vec<Message>,
            ) -> Result<CommandResult, String>
            + Send
            + Sync
            + 'static,
    {
        let cmd = SyncCommand {
            name_str: name,
            description_str: description,
            usage_str: name,
            aliases_str: &[],
            handler,
        };
        self.register(cmd);
    }

    /// Register a synchronous command with a custom usage string.
    pub fn register_sync_with_usage<F>(
        &mut self,
        name: &'static str,
        description: &'static str,
        usage: &'static str,
        handler: F,
    ) where
        F: Fn(
                Option<&str>,
                &mut CommandContext,
                &mut Vec<Message>,
            ) -> Result<CommandResult, String>
            + Send
            + Sync
            + 'static,
    {
        let cmd = SyncCommand {
            name_str: name,
            description_str: description,
            usage_str: usage,
            aliases_str: &[],
            handler,
        };
        self.register(cmd);
    }

    /// Parse user input and dispatch to the appropriate command handler.
    ///
    /// Returns `Err(msg)` if the command is unknown or the handler fails.
    pub async fn dispatch(
        &self,
        input: &str,
        ctx: &mut CommandContext,
        messages: &mut Vec<Message>,
    ) -> Result<CommandResult, String> {
        let input = input.trim();
        if !input.starts_with('/') {
            return Err("Input is not a command".to_string());
        }

        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd_name = parts[0].to_lowercase();
        let raw_arg = parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());

        // Resolve aliases
        let (resolved_name, effective_arg) =
            if let Some(&target) = self.aliases.get(cmd_name.as_str()) {
                let arg = self
                    .alias_fixed_args
                    .get(cmd_name.as_str())
                    .copied()
                    .or(raw_arg);
                (target, arg)
            } else {
                (cmd_name.as_str(), raw_arg)
            };

        let handler = self
            .commands
            .get(resolved_name)
            .ok_or_else(|| format!("Unknown command: {}", cmd_name))?;

        // Populate command descriptions for /help
        ctx.command_descriptions = self.command_descriptions();

        handler.execute(effective_arg, ctx, messages).await
    }

    /// Get all command names (primary + aliases), sorted for display.
    pub fn command_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.commands.keys().copied().collect();
        names.extend(self.aliases.keys().copied());
        names.sort();
        names
    }

    /// Get (usage, description) pairs for /help display.
    ///
    /// Returns entries from registered commands plus alias entries.
    pub fn command_descriptions(&self) -> Vec<(&'static str, &'static str)> {
        let mut descs: Vec<(&'static str, &'static str)> = self
            .commands
            .values()
            .map(|cmd| (cmd.usage(), cmd.description()))
            .collect();

        // Add alias entries
        for (&alias, &target) in &self.aliases {
            if let Some(cmd) = self.commands.get(target) {
                let desc = cmd.description();
                // Check if this alias has a custom description stored
                descs.push((alias, desc));
            }
        }

        descs.sort_by(|a, b| a.0.cmp(b.0));
        descs
    }

    /// Check if a command name (or alias) is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.commands.contains_key(name) || self.aliases.contains_key(name)
    }
}
