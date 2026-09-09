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

/// Built-in tools that may proceed without an explicit confirmation prompt
/// in sandbox mode.
///
/// `write`/`edit` qualify because their path arguments are statically
/// confined to the sandbox root before execution. The read-only tools qualify
/// because their paths are checked at execution time (ls/read/grep/glob) or
/// they touch no filesystem at all (web_search/web_fetch/screenshot).
///
/// Anything not on this list — `run`, signal tools, and all custom tools —
/// runs shell commands that cannot be statically contained, so sandbox mode
/// forces an explicit confirmation for them even under auto-accept.
const SANDBOX_AUTO_APPROVABLE: &[&str] = &[
    "ls",
    "read",
    "grep",
    "glob",
    "web_search",
    "web_fetch",
    "screenshot",
    "write",
    "edit",
];

/// Determine whether a tool call should be approved, needs user confirmation,
/// or should be denied.
///
/// This is pure logic — no I/O. The caller is responsible for implementing
/// the user interaction when `NeedsConfirmation` is returned.
///
/// The logic follows these rules:
/// 0. Sandbox mode: only tools in `SANDBOX_AUTO_APPROVABLE` (built-ins with
///    contained paths or no filesystem access) may proceed without a prompt.
///    `run` and any custom tool execute shell commands that cannot be
///    statically contained, so they always require confirmation — even with
///    auto-accept enabled or for custom read-only tools.
/// 1. Read-only tools (no confirmation needed) → `AutoApproved { auto_accepted: false }`
/// 2. Per-turn auto-accept (`auto_accept == true`) → `AutoApproved { auto_accepted: true }`
///    for everything except unsafe `run` commands (which prompt via `NeedsConfirmation`).
/// 3. Auto-accept mode (All) → `AutoApproved { auto_accepted: true }` for everything.
/// 4. Auto-accept mode (Safe) → auto-approve safe `run` commands;
///    unsafe `run` and other destructive tools need confirmation.
/// 5. Everything else → `NeedsConfirmation`
pub fn decide_tool_confirmation(
    call: &ToolCall,
    auto_accept: bool,
    auto_accept_mode: AutoAcceptMode,
    safe_commands: &[String],
    denied_commands: &[String],
    needs_confirmation: bool,
    sandbox_active: bool,
) -> ConfirmationDecision {
    // Sandbox mode gate: runs BEFORE the read-only early return so that
    // custom read-only tools (arbitrary shell commands) also require an
    // explicit confirmation. Built-ins on the allow-list proceed normally.
    if sandbox_active && !SANDBOX_AUTO_APPROVABLE.contains(&call.function.name.as_str()) {
        return ConfirmationDecision::NeedsConfirmation;
    }

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
        let decision = decide_tool_confirmation(
            &call,
            false,
            AutoAcceptMode::Off,
            &[],
            &[],
            false, // needs_confirmation = false → read-only
            false,
        );
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
        let decision = decide_tool_confirmation(
            &call,
            false,
            AutoAcceptMode::Safe,
            &[],
            &[],
            true, // needs_confirmation = true → destructive
            false,
        );
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn all_mode_auto_approves_destructive() {
        let call = make_call("write", json!({"path": "/tmp/file", "content": "hi"}));
        let decision =
            decide_tool_confirmation(&call, false, AutoAcceptMode::All, &[], &[], true, false);
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
            false,
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
            false,
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
            false,
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
            false,
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
            false,
        );
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn off_mode_always_prompts() {
        let call = make_call("write", json!({"path": "/tmp/file", "content": "hi"}));
        let decision =
            decide_tool_confirmation(&call, false, AutoAcceptMode::Off, &[], &[], true, false);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    // ── Sandbox mode tests ──────────────────────────────────────────────

    #[test]
    fn sandbox_run_always_prompts_even_in_all_mode() {
        let call = make_call("run", json!({"command": "ls"}));
        let decision =
            decide_tool_confirmation(&call, false, AutoAcceptMode::All, &[], &[], true, true);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn sandbox_run_always_prompts_with_per_turn_auto_accept() {
        let call = make_call("run", json!({"command": "ls"}));
        let decision =
            decide_tool_confirmation(&call, true, AutoAcceptMode::Off, &[], &[], true, true);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn sandbox_write_can_be_auto_approved() {
        let call = make_call("write", json!({"path": "./file", "content": "hi"}));
        let decision =
            decide_tool_confirmation(&call, true, AutoAcceptMode::All, &[], &[], true, true);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn sandbox_edit_can_be_auto_approved() {
        let call = make_call(
            "edit",
            json!({"path": "./file", "old_str": "a", "new_str": "b"}),
        );
        let decision =
            decide_tool_confirmation(&call, true, AutoAcceptMode::All, &[], &[], true, true);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: true
            }
        );
    }

    #[test]
    fn sandbox_read_only_tools_unaffected() {
        let call = make_call("read", json!({"path": "./file"}));
        let decision =
            decide_tool_confirmation(&call, false, AutoAcceptMode::Off, &[], &[], false, true);
        assert_eq!(
            decision,
            ConfirmationDecision::AutoApproved {
                auto_accepted: false
            }
        );
    }

    #[test]
    fn sandbox_custom_readonly_tool_always_prompts() {
        // Custom tools are shell commands — cannot be contained statically,
        // so sandbox mode forces confirmation even if they're read-only and
        // auto-accept mode is All.
        let call = make_call("my_custom_tool", json!({"arg": "x"}));
        let decision =
            decide_tool_confirmation(&call, true, AutoAcceptMode::All, &[], &[], false, true);
        assert_eq!(decision, ConfirmationDecision::NeedsConfirmation);
    }

    #[test]
    fn sandbox_builtin_readonly_tools_unaffected() {
        for name in [
            "ls",
            "grep",
            "glob",
            "web_search",
            "web_fetch",
            "screenshot",
        ] {
            let call = make_call(name, json!({}));
            let decision =
                decide_tool_confirmation(&call, false, AutoAcceptMode::All, &[], &[], false, true);
            assert_eq!(
                decision,
                ConfirmationDecision::AutoApproved {
                    auto_accepted: false
                },
                "tool {} should not prompt in sandbox mode",
                name
            );
        }
    }
}
