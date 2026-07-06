use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::shell::ShellCommand;

/// Lifecycle events that hooks can listen to.
///
/// Hooks fire at specific points during the agent loop:
///
/// | Event | When it fires | Context available |
/// |-------|-------------|-------------------|
/// | `BeforeUserMessage` | After input is read, before pushing to messages | `user_input` |
/// | `AfterUserMessage` | After user message is pushed | `user_input`, `message_count` |
/// | `BeforeLlmCall` | Before `provider.chat()` | `message_count` |
/// | `AfterLlmResponse` | After LLM streaming completes | `response_content`, `message_count` |
/// | `BeforeToolCall` | Before each tool execution | `tool_name`, `tool_args` |
/// | `AfterToolCall` | After each tool returns | `tool_name`, `tool_args`, `tool_result` |
/// | `OnExit` | When the agent loop exits | `message_count` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    BeforeUserMessage,
    AfterUserMessage,
    BeforeLlmCall,
    AfterLlmResponse,
    BeforeToolCall,
    AfterToolCall,
    OnExit,
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookEvent::BeforeUserMessage => f.write_str("before_user_message"),
            HookEvent::AfterUserMessage => f.write_str("after_user_message"),
            HookEvent::BeforeLlmCall => f.write_str("before_llm_call"),
            HookEvent::AfterLlmResponse => f.write_str("after_llm_response"),
            HookEvent::BeforeToolCall => f.write_str("before_tool_call"),
            HookEvent::AfterToolCall => f.write_str("after_tool_call"),
            HookEvent::OnExit => f.write_str("on_exit"),
        }
    }
}

impl std::str::FromStr for HookEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "before_user_message" => Ok(HookEvent::BeforeUserMessage),
            "after_user_message" => Ok(HookEvent::AfterUserMessage),
            "before_llm_call" => Ok(HookEvent::BeforeLlmCall),
            "after_llm_response" => Ok(HookEvent::AfterLlmResponse),
            "before_tool_call" => Ok(HookEvent::BeforeToolCall),
            "after_tool_call" => Ok(HookEvent::AfterToolCall),
            "on_exit" => Ok(HookEvent::OnExit),
            other => Err(format!(
                "Unknown hook event '{}'. Valid: before_user_message, after_user_message, before_llm_call, after_llm_response, before_tool_call, after_tool_call, on_exit",
                other
            )),
        }
    }
}

/// Definition of a hook from the plugin config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Human-readable name for the hook (used in logs and warnings).
    pub name: String,
    /// The lifecycle event this hook fires on.
    pub event: HookEvent,
    /// Shell command template to execute. Supports `{var}` placeholders
    /// that are substituted from the hook context.
    #[serde(flatten)]
    pub command: ShellCommand,
    /// If true, the stdout of this hook is collected and injected into the
    /// conversation as a system message (for `before_llm_call`) or appended
    /// to the user input (for `before_user_message`). Default: false.
    #[serde(default)]
    pub inject_stdout: bool,
    /// If set, when the hook's stdout starts with this string, the action
    /// is blocked (e.g. a `before_tool_call` hook can block a tool call).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_on_output: Option<String>,
}

/// Runtime context passed to hooks. Only the fields relevant to the
/// current event are populated; others are `None`.
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// User input text (available for `before_user_message`, `after_user_message`).
    pub user_input: Option<String>,
    /// Total number of messages in the conversation (available for most events).
    pub message_count: Option<usize>,
    /// LLM response content (available for `after_llm_response`).
    pub response_content: Option<String>,
    /// Tool name being called (available for `before_tool_call`, `after_tool_call`).
    pub tool_name: Option<String>,
    /// Tool arguments as JSON (available for `before_tool_call`, `after_tool_call`).
    pub tool_args: Option<serde_json::Value>,
    /// Tool result string (available for `after_tool_call`).
    pub tool_result: Option<String>,
    /// Session ID (available for all events).
    pub session_id: Option<String>,
}

impl HookContext {
    /// Convert the context to a `HashMap` of variable names → values,
    /// used for `{var}` template substitution and `TH_*` environment variables.
    pub fn to_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        if let Some(ref input) = self.user_input {
            vars.insert("user_input".to_string(), input.clone());
        }
        if let Some(count) = self.message_count {
            vars.insert("message_count".to_string(), count.to_string());
        }
        if let Some(ref resp) = self.response_content {
            vars.insert("response".to_string(), resp.clone());
        }
        if let Some(ref name) = self.tool_name {
            vars.insert("tool".to_string(), name.clone());
        }
        if let Some(ref args) = self.tool_args {
            vars.insert("args".to_string(), args.to_string());
        }
        if let Some(ref result) = self.tool_result {
            vars.insert("result".to_string(), result.clone());
        }
        if let Some(ref id) = self.session_id {
            vars.insert("session_id".to_string(), id.clone());
        }
        vars
    }

    /// Build the context as `TH_*` environment variables for the shell command.
    pub fn to_env_vars(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Some(ref input) = self.user_input {
            env.push(("TH_USER_INPUT".to_string(), input.clone()));
        }
        if let Some(count) = self.message_count {
            env.push(("TH_MESSAGE_COUNT".to_string(), count.to_string()));
        }
        if let Some(ref resp) = self.response_content {
            env.push(("TH_RESPONSE".to_string(), resp.clone()));
        }
        if let Some(ref name) = self.tool_name {
            env.push(("TH_TOOL_NAME".to_string(), name.clone()));
        }
        if let Some(ref args) = self.tool_args {
            env.push(("TH_TOOL_ARGS".to_string(), args.to_string()));
        }
        if let Some(ref result) = self.tool_result {
            env.push(("TH_TOOL_RESULT".to_string(), result.clone()));
        }
        if let Some(ref id) = self.session_id {
            env.push(("TH_SESSION_ID".to_string(), id.clone()));
        }
        env
    }
}

/// Aggregated result from running hooks for a single event.
#[derive(Debug, Clone, Default)]
pub struct HookOutcome {
    /// Text collected from hooks with `inject_stdout: true`.
    /// Use this to inject context into the conversation.
    pub injected_text: Option<String>,
    /// True if any hook returned the `block_on_output` string.
    pub blocked: bool,
    /// The reason for blocking (from the hook's stdout).
    pub block_reason: Option<String>,
    /// Non-fatal errors/warnings from hook execution.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_serde() {
        let json = r#""before_tool_call""#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event, HookEvent::BeforeToolCall);

        let json = serde_json::to_string(&HookEvent::OnExit).unwrap();
        assert_eq!(json, r#""on_exit""#);
    }

    #[test]
    fn test_hook_event_from_str() {
        assert_eq!(
            "before_llm_call".parse::<HookEvent>().unwrap(),
            HookEvent::BeforeLlmCall
        );
        assert!("invalid".parse::<HookEvent>().is_err());
    }

    #[test]
    fn test_hook_definition_parses() {
        let json = r#"{
            "name": "logger",
            "event": "after_tool_call",
            "command": "echo '{tool}'",
            "timeout_secs": 5
        }"#;
        let hook: HookDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(hook.name, "logger");
        assert_eq!(hook.event, HookEvent::AfterToolCall);
        assert_eq!(hook.command.command, "echo '{tool}'");
        assert_eq!(hook.command.timeout_secs, 5);
        assert!(!hook.inject_stdout);
        assert!(hook.block_on_output.is_none());
    }

    #[test]
    fn test_hook_definition_with_inject_and_block() {
        let json = r#"{
            "name": "blocker",
            "event": "before_tool_call",
            "command": "echo check",
            "inject_stdout": true,
            "block_on_output": "BLOCKED"
        }"#;
        let hook: HookDefinition = serde_json::from_str(json).unwrap();
        assert!(hook.inject_stdout);
        assert_eq!(hook.block_on_output.as_deref(), Some("BLOCKED"));
    }

    #[test]
    fn test_hook_definition_default_timeout() {
        let json = r#"{
            "name": "test",
            "event": "on_exit",
            "command": "echo bye"
        }"#;
        let hook: HookDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(hook.command.timeout_secs, 30);
    }

    #[test]
    fn test_hook_context_to_vars() {
        let ctx = HookContext {
            tool_name: Some("run".to_string()),
            tool_args: Some(serde_json::json!({"command": "ls"})),
            tool_result: Some("file1\nfile2".to_string()),
            session_id: Some("abc123".to_string()),
            ..Default::default()
        };
        let vars = ctx.to_vars();
        assert_eq!(vars.get("tool"), Some(&"run".to_string()));
        assert_eq!(vars.get("result"), Some(&"file1\nfile2".to_string()));
        assert_eq!(vars.get("session_id"), Some(&"abc123".to_string()));
        assert!(!vars.contains_key("user_input"));
    }

    #[test]
    fn test_hook_context_to_env_vars() {
        let ctx = HookContext {
            user_input: Some("hello".to_string()),
            message_count: Some(5),
            ..Default::default()
        };
        let env = ctx.to_env_vars();
        let env_map: HashMap<String, String> = env.into_iter().collect();
        assert_eq!(env_map.get("TH_USER_INPUT"), Some(&"hello".to_string()));
        assert_eq!(env_map.get("TH_MESSAGE_COUNT"), Some(&"5".to_string()));
    }

    #[test]
    fn test_hook_context_empty() {
        let ctx = HookContext::default();
        assert!(ctx.to_vars().is_empty());
        assert!(ctx.to_env_vars().is_empty());
    }

    #[test]
    fn test_hook_outcome_default() {
        let outcome = HookOutcome::default();
        assert!(outcome.injected_text.is_none());
        assert!(!outcome.blocked);
        assert!(outcome.block_reason.is_none());
        assert!(outcome.warnings.is_empty());
    }
}
