//! Repair the assistant ↔ tool-call pairing in a conversation history.
//!
//! OpenAI-compatible servers (and the Azure/OpenAI Responses API behind
//! OpenRouter) reject a request with HTTP 400
//! `“No tool output found for function call <id>”` when the message list
//! contains an assistant `tool_calls` entry with no matching `tool` result, or
//! a `tool` result whose call was never declared. Strict providers also reject
//! a `tool` result carrying an empty/missing `tool_call_id`.
//!
//! Real histories grow holes in several ways: the user presses Ctrl+C between
//! the assistant message and its results, a signal tool answers with something
//! other than a `tool` message, `/compact` cuts the keep-window between a call
//! and its result, or a session resumed with `-c` was killed mid-turn. Rather
//! than patch every cause, [`sanitize_tool_pairs`] repairs the invariant at the
//! one place that sees the exact shape of the outgoing request.
//!
//! The transform is deliberately conservative: a message that is already
//! correctly paired is returned untouched, and the only content ever *invented*
//! is a placeholder result for a call that has none.

use std::collections::{HashMap, HashSet};

use crate::provider::{Message, Role, ToolCall};

/// Placeholder content for a declared tool call that has no recorded result.
///
/// Phrased so the model understands the call produced nothing, instead of
/// treating its absence as a successful (empty) result.
fn missing_result_content(tool_name: &str) -> String {
    format!(
        "[TinyHarness] No output was recorded for this call to '{tool_name}' — the turn was \
         aborted before the tool produced a result. Treat the call as never having run, and ask \
         the user before repeating it."
    )
}

/// Build a placeholder `tool` result for `id` / `tool_name`.
fn placeholder_result(id: &str, tool_name: &str) -> Message {
    Message {
        role: Role::Tool,
        content: missing_result_content(tool_name),
        tool_calls: vec![],
        tool_call_id: Some(id.to_string()),
        images: vec![],
        thinking: None,
    }
}

/// What a tool-pair repair changed in a history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolPairFixes {
    /// How many declared calls had no result and received a placeholder.
    pub stubbed: usize,
    /// How many tool results were dropped — because no declared call owned
    /// them, or because they duplicated the result of an already-answered call.
    pub dropped: usize,
}

impl ToolPairFixes {
    /// Whether the repair changed the history at all.
    pub fn changed(&self) -> bool {
        self.stubbed > 0 || self.dropped > 0
    }
}

/// A tool call declared by an assistant message.
struct DeclaredCall {
    id: String,
    name: String,
    /// Index of the owning assistant message in the normalized history.
    owner: usize,
}

/// Return a copy of `messages` in which every assistant tool call has exactly
/// one matching `tool` result, and every `tool` result matches a declared call.
///
/// Three repairs are applied:
///
/// 1. **Unusable ids** — a tool call with a missing, empty, or repeated id gets
///    a synthesized unique one, echoed consistently in the assistant message and
///    in its result. Two calls sharing an id can never both be answered.
/// 2. **Unanswered calls** — a placeholder result is inserted directly after the
///    assistant message that declared the call.
/// 3. **Orphan / duplicate results** — a `tool` message whose `tool_call_id`
///    matches no declared call, or that repeats an already-answered call, is
///    dropped. A `tool` message with *no* id is not treated as an orphan: it is
///    bound to the oldest declared call still awaiting a result, which keeps
///    providers that omit ids (plain Ollama, local servers) working.
///
/// Ordering is otherwise preserved, so a correctly paired history comes back
/// unchanged with zeroed [`ToolPairFixes`].
pub fn sanitize_tool_pairs(messages: &[Message]) -> (Vec<Message>, ToolPairFixes) {
    // ── Pass 1: normalize call ids and index every declared call ────────────
    //
    // `declared` keeps declaration order (needed for the positional binding in
    // pass 2) and records the owning assistant message plus the tool name, both
    // required to place and word a placeholder.
    let mut declared: Vec<DeclaredCall> = Vec::new();
    let mut declared_ids: HashSet<String> = HashSet::new();
    let mut normalized: Vec<Message> = Vec::with_capacity(messages.len());
    let mut next_synthetic = 0_usize;

    for message in messages {
        let mut message = message.clone();
        if message.role == Role::Assistant && !message.tool_calls.is_empty() {
            let owner = normalized.len();
            for call in &mut message.tool_calls {
                let id = match usable_id(call) {
                    Some(candidate) if !declared_ids.contains(&candidate) => candidate,
                    _ => {
                        let id = format!("call_fixed_{next_synthetic}");
                        next_synthetic += 1;
                        id
                    }
                };
                // Write the id back onto the call so the assistant message and
                // its result agree — a placeholder keyed to an id the assistant
                // never declares is exactly the mismatch we are fixing.
                call.id = Some(id.clone());
                declared_ids.insert(id.clone());
                declared.push(DeclaredCall {
                    name: call.function.name.clone(),
                    id,
                    owner,
                });
            }
        }
        normalized.push(message);
    }

    // ── Pass 2: bind results to calls ──────────────────────────────────────
    //
    // `binding[msg_index]` is the id a result must carry; results absent from
    // the map are orphans or duplicates and get dropped in pass 3.
    let mut binding: HashMap<usize, String> = HashMap::new();
    let mut answered: HashSet<String> = HashSet::new();
    let mut dropped = 0_usize;
    let mut cursor = 0_usize;

    for (index, message) in normalized.iter().enumerate() {
        if message.role != Role::Tool {
            continue;
        }
        let claimed = match message.tool_call_id.as_deref() {
            Some(id) if !id.is_empty() => {
                // An id no message in this history declares belongs to a call
                // that was summarized away or cut out — keeping the result would
                // trip the very 400 this module exists to prevent.
                declared_ids.contains(id).then(|| id.to_string())
            }
            // No id: bind positionally to the oldest call still unanswered.
            _ => {
                while cursor < declared.len() && answered.contains(&declared[cursor].id) {
                    cursor += 1;
                }
                declared.get(cursor).map(|call| {
                    cursor += 1;
                    call.id.clone()
                })
            }
        };
        match claimed {
            Some(id) if answered.insert(id.clone()) => {
                binding.insert(index, id);
            }
            _ => dropped += 1,
        }
    }

    // ── Pass 3: emit, inserting a placeholder after each unanswered call ────
    let mut placeholders: HashMap<usize, Vec<DeclaredCall>> = HashMap::new();
    for call in declared {
        if !answered.contains(&call.id) {
            placeholders.entry(call.owner).or_default().push(call);
        }
    }
    let stubbed = placeholders.values().map(Vec::len).sum();

    let mut out: Vec<Message> = Vec::with_capacity(normalized.len() + stubbed);
    for (index, message) in normalized.into_iter().enumerate() {
        match binding.get(&index) {
            Some(id) => {
                let mut message = message;
                message.tool_call_id = Some(id.clone());
                out.push(message);
            }
            None if message.role == Role::Tool => {
                // Orphan or duplicate — already counted in `dropped`.
            }
            None => out.push(message),
        }
        if let Some(calls) = placeholders.get(&index) {
            out.extend(
                calls
                    .iter()
                    .map(|call| placeholder_result(&call.id, &call.name)),
            );
        }
    }

    (out, ToolPairFixes { stubbed, dropped })
}

/// Repair `messages` in place and report what changed. See
/// [`sanitize_tool_pairs`].
pub fn repair_tool_pairs(messages: &mut Vec<Message>) -> ToolPairFixes {
    let (repaired, fixes) = sanitize_tool_pairs(messages);
    *messages = repaired;
    fixes
}

/// The id of `call`, if it is present and non-empty.
fn usable_id(call: &ToolCall) -> Option<String> {
    call.id.clone().filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolCallFunction;
    use serde_json::json;

    fn call(id: Option<&str>, name: &str) -> ToolCall {
        ToolCall {
            id: id.map(str::to_string),
            function: ToolCallFunction {
                name: name.to_string(),
                arguments: json!({}),
                thought_signature: None,
            },
        }
    }

    fn assistant(calls: Vec<ToolCall>) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: calls,
            tool_call_id: None,
            images: vec![],
            thinking: None,
        }
    }

    fn tool(id: Option<&str>, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: content.to_string(),
            tool_calls: vec![],
            tool_call_id: id.map(str::to_string),
            images: vec![],
            thinking: None,
        }
    }

    fn user(content: &str) -> Message {
        Message::simple(Role::User, content)
    }

    fn system() -> Message {
        Message::simple(Role::System, "sys")
    }

    fn ids_of(message: &Message) -> Vec<Option<String>> {
        match message.role {
            Role::Tool => vec![message.tool_call_id.clone()],
            _ => message
                .tool_calls
                .iter()
                .map(|call| call.id.clone())
                .collect(),
        }
    }

    /// Every declared call has exactly one result, and every result has a
    /// declared call — the invariant strict providers enforce.
    fn assert_paired(messages: &[Message]) {
        let mut declared: Vec<String> = Vec::new();
        for message in messages {
            if message.role == Role::Assistant {
                declared.extend(message.tool_calls.iter().filter_map(|call| call.id.clone()));
            }
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for message in messages {
            if message.role != Role::Tool {
                continue;
            }
            let id = message
                .tool_call_id
                .as_deref()
                .expect("tool result must carry an id");
            assert!(!id.is_empty(), "tool result id must not be empty");
            assert!(
                declared.iter().any(|declared| declared == id),
                "result {id} has no declared call"
            );
            assert!(seen.insert(id), "call {id} answered twice");
        }
        assert_eq!(
            seen.len(),
            declared.len(),
            "every declared call must be answered"
        );
    }

    #[test]
    fn correctly_paired_history_is_unchanged() {
        let messages = vec![
            system(),
            user("hi"),
            assistant(vec![call(Some("call_a"), "read")]),
            tool(Some("call_a"), "contents"),
            assistant(vec![
                call(Some("call_b"), "ls"),
                call(Some("call_c"), "glob"),
            ]),
            tool(Some("call_b"), "entries"),
            tool(Some("call_c"), "files"),
            Message::simple(Role::Assistant, "done"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert!(!fixes.changed());
        assert_eq!(out.len(), messages.len());
        for (before, after) in messages.iter().zip(&out) {
            assert_eq!(before.role, after.role);
            assert_eq!(before.content, after.content);
            assert_eq!(ids_of(before), ids_of(after));
        }
    }

    #[test]
    fn unanswered_call_gets_placeholder() {
        let messages = vec![
            system(),
            user("hi"),
            assistant(vec![call(Some("call_2ab5e00d"), "edit")]),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert_eq!(
            fixes,
            ToolPairFixes {
                stubbed: 1,
                dropped: 0
            }
        );
        assert_paired(&out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[3].role, Role::Tool);
        assert_eq!(out[3].tool_call_id.as_deref(), Some("call_2ab5e00d"));
        assert!(out[3].content.contains("edit"));
        assert!(out[3].content.contains("No output was recorded"));
    }

    #[test]
    fn orphan_result_is_dropped() {
        // What a truncated keep-window looks like after `/compact`: the owner
        // of `call_gone` was summarized away.
        let messages = vec![
            system(),
            tool(Some("call_gone"), "stale output"),
            user("keep going"),
            Message::simple(Role::Assistant, "sure"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert_eq!(
            fixes,
            ToolPairFixes {
                stubbed: 0,
                dropped: 1
            }
        );
        assert_eq!(out.len(), 3);
        assert!(!out.iter().any(|m| m.content == "stale output"));
        assert_paired(&out);
    }

    #[test]
    fn duplicate_result_is_dropped_but_first_wins() {
        let messages = vec![
            system(),
            assistant(vec![call(Some("call_a"), "read")]),
            tool(Some("call_a"), "real output"),
            tool(Some("call_a"), "second output"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert_eq!(
            fixes,
            ToolPairFixes {
                stubbed: 0,
                dropped: 1
            }
        );
        assert_paired(&out);
        let result = out
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("one result");
        assert_eq!(result.content, "real output");
    }

    #[test]
    fn missing_call_id_is_synthesized_and_echoed_in_placeholder() {
        let messages = vec![
            system(),
            assistant(vec![call(None, "grep"), call(Some(""), "glob")]),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert_eq!(fixes.stubbed, 2);
        assert_paired(&out);
        let assistant = &out[1];
        let first = assistant.tool_calls[0].id.as_deref().unwrap();
        let second = assistant.tool_calls[1].id.as_deref().unwrap();
        assert_eq!(first, "call_fixed_0");
        assert_eq!(second, "call_fixed_1");
        assert_eq!(out[2].tool_call_id.as_deref(), Some(first));
        assert_eq!(out[3].tool_call_id.as_deref(), Some(second));
    }

    #[test]
    fn result_without_id_binds_positionally() {
        // Local/Ollama histories carry no ids at all.
        let messages = vec![
            system(),
            assistant(vec![call(None, "read"), call(None, "ls")]),
            tool(None, "file contents"),
            tool(None, "directory entries"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert!(!fixes.changed());
        assert_paired(&out);
        let results: Vec<&Message> = out.iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(results[0].content, "file contents");
        assert_eq!(results[1].content, "directory entries");
        assert_eq!(results[0].tool_call_id.as_deref(), Some("call_fixed_0"));
        assert_eq!(results[1].tool_call_id.as_deref(), Some("call_fixed_1"));
    }

    #[test]
    fn idless_result_binds_only_to_unanswered_calls() {
        // `call_a` is already answered, so the id-less result must go to
        // `call_b` rather than shadowing `call_a`.
        let messages = vec![
            system(),
            assistant(vec![
                call(Some("call_a"), "read"),
                call(Some("call_b"), "ls"),
            ]),
            tool(Some("call_a"), "first"),
            tool(None, "second"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert!(!fixes.changed());
        assert_paired(&out);
        let results: Vec<&Message> = out.iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(results[1].tool_call_id.as_deref(), Some("call_b"));
        assert_eq!(results[1].content, "second");
    }

    #[test]
    fn result_scattered_after_an_intervening_message_is_kept() {
        // A call can be answered and then followed by an injected user message
        // (the `/use` path). Later results must still survive, and no
        // placeholder must appear.
        let messages = vec![
            system(),
            assistant(vec![
                call(Some("call_s"), "invoke_skill"),
                call(Some("call_r"), "read"),
            ]),
            tool(Some("call_s"), "skill activated"),
            user("/use review"),
            tool(Some("call_r"), "file contents"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert!(!fixes.changed());
        assert_eq!(out.len(), 5);
        assert_paired(&out);
    }

    #[test]
    fn placeholder_lands_next_to_its_owner() {
        let messages = vec![
            system(),
            assistant(vec![call(Some("call_x"), "run")]),
            user("actually, stop"),
            Message::simple(Role::Assistant, "ok"),
        ];

        let (out, fixes) = sanitize_tool_pairs(&messages);

        assert_eq!(fixes.stubbed, 1);
        assert_paired(&out);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[2].role, Role::Tool);
        assert_eq!(out[2].tool_call_id.as_deref(), Some("call_x"));
        assert_eq!(out[3].role, Role::User);
    }

    #[test]
    fn duplicate_call_ids_are_made_unique() {
        // Two calls sharing an id can never both be answered, so the second is
        // renamed and its now-unmatched result becomes a placeholder.
        let messages = vec![
            system(),
            assistant(vec![
                call(Some("call_a"), "read"),
                call(Some("call_a"), "ls"),
            ]),
            tool(Some("call_a"), "first output"),
            tool(Some("call_a"), "second output"),
        ];

        let (out, _) = sanitize_tool_pairs(&messages);

        assert_paired(&out);
        let assistant = &out[1];
        assert_ne!(
            assistant.tool_calls[0].id, assistant.tool_calls[1].id,
            "ids must be unique"
        );
        assert_eq!(assistant.tool_calls[1].id.as_deref(), Some("call_fixed_0"));
    }

    #[test]
    fn empty_history_round_trips() {
        let (out, fixes) = sanitize_tool_pairs(&[]);
        assert!(out.is_empty());
        assert!(!fixes.changed());
    }

    #[test]
    fn system_prompt_stays_first() {
        let messages = vec![system(), assistant(vec![call(Some("call_a"), "read")])];

        let (out, _) = sanitize_tool_pairs(&messages);

        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[0].content, "sys");
        assert_paired(&out);
    }

    #[test]
    fn repair_is_idempotent() {
        let messages = vec![
            system(),
            assistant(vec![call(Some("call_a"), "read")]),
            tool(Some("call_b"), "orphan"),
            user("next"),
            assistant(vec![call(Some("call_c"), "write")]),
        ];

        let (once, first_fixes) = sanitize_tool_pairs(&messages);
        assert_paired(&once);
        assert!(first_fixes.changed());
        let (twice, second_fixes) = sanitize_tool_pairs(&once);
        assert!(!second_fixes.changed(), "a repaired history is stable");
        assert_eq!(twice.len(), once.len());
    }

    #[test]
    fn in_place_repair_matches_sanitize() {
        let messages = vec![system(), assistant(vec![call(Some("call_a"), "edit")])];
        let expected_len = sanitize_tool_pairs(&messages).0.len();
        let mut repaired = messages.clone();

        let fixes = repair_tool_pairs(&mut repaired);

        assert_eq!(fixes.stubbed, 1);
        assert_eq!(repaired.len(), expected_len);
        assert_paired(&repaired);
    }
}
