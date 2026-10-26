// ── Tool Confirmation Logic ────────────────────────────────────────────────
//
// The decision tree for whether a tool call is approved lives here. At the
// "ask user" step, the CLI uses an interactive terminal prompt.
//
// This module extracts the pure decision logic so the loop only needs to
// implement the I/O part.

use tinyharness_lib::config::AutoAcceptMode;
use tinyharness_lib::provider::ToolCall;

use super::safety::is_safe_command;

/// Decision about whether a tool call is allowed to proceed.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmationDecision {
    /// The tool call is automatically approved (no user interaction needed).
    /// `auto_accepted` indicates whether this came from auto-accept mode
    /// (affects display: "Executing..." vs "(auto-accepted)").
    AutoApproved { auto_accepted: bool },

    /// The tool call needs explicit user confirmation.
    NeedsConfirmation,

    /// The tool call was denied (e.g., by a previous user response).
    /// This variant is not produced by `decide_tool_confirmation` directly,
    /// but is useful for callers that receive a "no" from the user.
    Denied,
}

/// Determine whether a tool call should be approved, needs user confirmation,
/// or should be denied.
///
/// This is pure logic — no I/O. The caller is responsible for implementing
/// the user interaction when `NeedsConfirmation` is returned.
///
/// The logic follows these rules:
/// 1. Read-only tools (no confirmation needed) → `AutoApproved { auto_accepted: false }`
/// 2. Per-turn auto-accept (`auto_accept == true`) → `AutoApproved { auto_accepted: true }`
///    for everything except unsafe `run` commands (which prompt via `NeedsConfirmation`).
/// 3. Auto-accept mode (All) → `AutoApproved { auto_accepted: true }` for everything.
/// 4. Auto-accept mode (Safe) → auto-approve safe `run` commands;
///    unsafe `run` and other destructive tools need confirmation.
/// 5. Everything else → `NeedsConfirmation`
///
/// Sandboxing does not feature here: shell commands are confined by the OS
/// (`--sandbox`) and the built-in file tools by the path guard, both enforced
/// at execution time, so approval follows the ordinary auto-accept policy
/// either way.
pub fn decide_tool_confirmation(
    call: &ToolCall,
    auto_accept: bool,
    auto_accept_mode: AutoAcceptMode,
    safe_commands: &[String],
    denied_commands: &[String],
    needs_confirmation: bool,
) -> ConfirmationDecision {
    // Read-only tools: always approved, never "auto-accepted"
    if !needs_confirmation {
        return ConfirmationDecision::AutoApproved {
            auto_accepted: false,
        };
    }

    // Per-turn auto-accept ('a' key): approve destructive tools for the rest
    // of this turn, but still prompt for unsafe `run` commands.
    if auto_accept
        && call.function.name == "run"
        && let Some(cmd_value) = call.function.arguments.get("command")
        && let Some(cmd_str) = cmd_value.as_str()
        && !is_safe_command(cmd_str, safe_commands, denied_commands)
    {
        return ConfirmationDecision::NeedsConfirmation;
    }
    if auto_accept {
        return ConfirmationDecision::AutoApproved {
            auto_accepted: true,
        };
    }

    match auto_accept_mode {
        AutoAcceptMode::All => {
            // Auto-accept all mode: approve everything without prompting
            ConfirmationDecision::AutoApproved {
                auto_accepted: true,
            }
        }
        AutoAcceptMode::Safe => {
            // Safe mode: auto-approve safe run commands, but prompt for
            // unsafe run and other destructive tools.
            if call.function.name == "run"
                && let Some(cmd_value) = call.function.arguments.get("command")
                && let Some(cmd_str) = cmd_value.as_str()
                && is_safe_command(cmd_str, safe_commands, denied_commands)
            {
                return ConfirmationDecision::AutoApproved {
                    auto_accepted: true,
                };
            }
            // Unsafe run or other destructive tools — need user confirmation
            ConfirmationDecision::NeedsConfirmation
        }
        AutoAcceptMode::Off => {
            // Auto-accept off: always prompt for destructive tools
            ConfirmationDecision::NeedsConfirmation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: Some("test-id".to_string()),
            function: tinyharness_lib::provider::ToolCallFunction {
                name: name.to_string(),
                arguments: args,
                thought_signature: None,
            },
        }
    }

    #[test]
    fn read_only_always_approved() {
        let call = make_call("read", json!({"path": "/tmp/file"}));
        let decision = decide_tool_confirmation(&call, false, AutoAcceptMode::Off, &[], &[], false);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: false
            }
        );
    }

    #[test]
    fn safe_mode_prompts_for_destructive() {
        let call = make_call("write", json!({"path": "/tmp/file", "content": "hi"}));
        let decision = decide_tool_confirmation(&call, false, AutoAcceptMode::Safe, &[], &[], true);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn all_mode_auto_approves_destructive() {
        let call = make_call("write", json!({"path": "/tmp/file", "content": "hi"}));
        let decision = decide_tool_confirmation(&call, false, AutoAcceptMode::All, &[], &[], true);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn per_turn_auto_accept_overrides() {
        let call = make_call("write", json!({"path": "/tmp/file", "content": "hi"}));
        let decision = decide_tool_confirmation(
            &call,
            true,                // auto_accept = true
            AutoAcceptMode::Off, // mode is Off, but per-turn overrides
            &[],
            &[],
            true,
        );
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn per_turn_auto_accept_prompts_unsafe_run() {
        let call = make_call("run", json!({"command": "rm -rf /"}));
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        let decision = decide_tool_confirmation(
            &call,
            true, // auto_accept = true
            AutoAcceptMode::Off,
            &safe_commands,
            &[],
            true,
        );
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn per_turn_auto_accept_approves_safe_run() {
        let call = make_call("run", json!({"command": "ls -la"}));
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        let decision = decide_tool_confirmation(
            &call,
            true, // auto_accept = true
            AutoAcceptMode::Off,
            &safe_commands,
            &[],
            true,
        );
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn safe_mode_auto_approves_safe_run() {
        let call = make_call("run", json!({"command": "ls -la"}));
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        let decision = decide_tool_confirmation(
            &call,
            false,
            AutoAcceptMode::Safe,
            &safe_commands,
            &[],
            true,
        );
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn safe_mode_prompts_unsafe_run() {
        let call = make_call("run", json!({"command": "rm -rf /"}));
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        let decision = decide_tool_confirmation(
            &call,
            false,
            AutoAcceptMode::Safe,
            &safe_commands,
            &[],
            true,
        );
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn off_mode_always_prompts() {
        let call = make_call("write", json!({"path": "/tmp/file", "content": "hi"}));
        let decision = decide_tool_confirmation(&call, false, AutoAcceptMode::Off, &[], &[], true);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    // ── Sandbox interaction tests ───────────────────────────────────────
    //
    // Sandboxing is enforced at execution time (OS sandbox for shell tools,
    // path guard for file tools), so it must not change the approval
    // decision. These pin that down: the same inputs that pass without a
    // sandbox pass with one.

    #[test]
    fn sandboxing_does_not_force_confirmation() {
        // `run` under auto-accept All is approved — the command is confined
        // by the OS sandbox, so an extra prompt would add nothing.
        let call = make_call("run", json!({"command": "ls"}));
        let decision = decide_tool_confirmation(&call, false, AutoAcceptMode::All, &[], &[], true);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn sandboxing_does_not_bypass_the_safe_list() {
        // An unsafe command still prompts, sandboxed or not.
        let safe = tinyharness_lib::config::get_default_safe_commands();
        let call = make_call("run", json!({"command": "rm -rf /"}));
        let decision =
            decide_tool_confirmation(&call, false, AutoAcceptMode::Safe, &safe, &[], true);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn custom_tools_follow_the_normal_policy() {
        // Custom tools used to always prompt under --sandbox because they
        // could not be contained; now they are sandboxed like `run`, so a
        // read-only one (needs_confirmation = false) is approved silently.
        let call = make_call("my_custom_tool", json!({"arg": "x"}));
        let decision = decide_tool_confirmation(&call, false, AutoAcceptMode::Off, &[], &[], false);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: false
            }
        );
    }
}
