use std::io::Write;

use tinyharness_lib::session::{SessionMeta, SessionStore, format_age};
use tinyharness_ui::output::Output;

use tinyharness_ui::style::*;

/// Format a session for display in the `/sessions` listing.
fn format_session_list(sessions: &[SessionMeta], current_id: Option<&str>) -> String {
    let mut output = String::new();

    if sessions.is_empty() {
        output.push_str(&format!("{ORANGE}No sessions found.{RESET}"));
        return output;
    }

    output.push_str(&format!(
        "{BOLD}Available sessions (most recent first):{RESET}\n\n",
    ));

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for meta in sessions {
        let is_current = current_id == Some(meta.id.as_str());
        let marker = if is_current {
            format!("{CYAN}▸{RESET}")
        } else {
            " ".to_string()
        };

        let age = format_age(now_secs.saturating_sub(meta.updated_at));
        let name_str = meta.name.as_deref().unwrap_or("unnamed");

        // Truncate working_dir for display
        let dir_display = if meta.working_dir.len() > 40 {
            let end = meta.working_dir.len().saturating_sub(37);
            let start = meta.working_dir.floor_char_boundary(end);
            format!("...{}", &meta.working_dir[start..])
        } else {
            meta.working_dir.clone()
        };

        output.push_str(&format!(
            "{marker} {BLUE}{}{RESET} — {BOLD}{name_str}{RESET}\n",
            &meta.id[..12],
        ));
        output.push_str(&format!(
            "  {marker}  {GRAY}{} msgs, {ITALIC}{age}{RESET}  {GRAY}{dir_display}\n",
            meta.message_count,
        ));
    }

    output
}

/// Delete empty sessions (0 messages) automatically.
/// Returns the number of sessions deleted.
pub fn cleanup_empty_sessions() -> usize {
    let store = SessionStore::default_path();
    let sessions = store.list_all();
    let mut deleted = 0;

    for meta in &sessions {
        if meta.message_count == 0 {
            if let Err(e) = store.delete(&meta.id) {
                let mut stderr = Output::stderr();
                let _ = writeln!(
                    stderr,
                    "{RED}Warning: Failed to delete empty session {}: {e}{RESET}",
                    &meta.id[..8],
                );
            } else {
                deleted += 1;
            }
        }
    }

    deleted
}

pub fn execute_list(out: &mut Output, current_session_id: Option<&str>) {
    let store = SessionStore::default_path();

    // Auto-delete empty sessions before listing
    let deleted = cleanup_empty_sessions();
    if deleted > 0 {
        let _ = writeln!(out, "{GRAY}ℹ Cleaned up {deleted} empty session(s){RESET}");
    }

    let sessions = store.list_all();
    let output = format_session_list(&sessions, current_session_id);
    let _ = writeln!(out, "{output}");
}

/// Delete a session by ID or name.
pub fn execute_delete(out: &mut Output, session_id: &str, current_session_id: Option<&str>) {
    let store = SessionStore::default_path();

    // Find the session
    let meta = store
        .list_all()
        .into_iter()
        .find(|s| s.id.starts_with(session_id) || s.name.as_ref().is_some_and(|n| n == session_id));

    let meta = match meta {
        Some(m) => m,
        None => {
            let _ = writeln!(out, "{RED}✗ Session '{session_id}' not found{RESET}");
            return;
        }
    };

    // Prevent deleting current session without warning
    let is_current = current_session_id == Some(meta.id.as_str());

    let name_str = meta.name.as_deref().unwrap_or("unnamed");
    let _ = writeln!(
        out,
        "{ORANGE}⚠ Delete session \"{name_str}\" ({} messages)? This cannot be undone.{RESET}",
        meta.message_count,
    );
    let _ = write!(out, "{BOLD}Type 'yes' to confirm: {RESET}");
    let _ = out.flush();

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        let _ = writeln!(out, "{RED}✗ Cancelled{RESET}");
        return;
    }

    if input.trim() != "yes" {
        let _ = writeln!(out, "{GRAY}✗ Cancelled{RESET}");
        return;
    }

    if let Err(e) = store.delete(&meta.id) {
        let _ = writeln!(out, "{RED}✗ Failed to delete session: {e}{RESET}");
        return;
    }

    let _ = writeln!(
        out,
        "{GREEN}✓ Deleted session {} — \"{name_str}\"{RESET}",
        &meta.id[..12],
    );

    // If we deleted the current session, warn the user
    if is_current {
        let _ = writeln!(
            out,
            "{ORANGE}⚠ You deleted the current session. Consider switching to another session or starting a new one.{RESET}",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::strip_ansi;
    use tinyharness_lib::mode::AgentMode;

    fn test_meta(id: &str, name: Option<&str>, msgs: usize) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            working_dir: "/tmp/project".to_string(),
            created_at: 1000,
            updated_at: 2000,
            mode: AgentMode::Agent,
            provider: "ollama".to_string(),
            model: Some("test-model".to_string()),
            name: name.map(|s| s.to_string()),
            message_count: msgs,
            token_usage: None,
            total_tool_calls: 0,
            total_tokens_used: 0,
        }
    }

    #[test]
    fn format_empty_sessions() {
        let output = format_session_list(&[], None);
        let plain = strip_ansi(&output);
        assert!(plain.contains("No sessions found"));
    }

    #[test]
    fn format_single_session() {
        let sessions = vec![test_meta("abcdef1234567890", Some("my-project"), 5)];
        let output = format_session_list(&sessions, None);
        let plain = strip_ansi(&output);
        assert!(plain.contains("Available sessions"));
        assert!(plain.contains("my-project"));
        assert!(plain.contains("abcdef123456"));
        assert!(plain.contains("5 msgs"));
    }

    #[test]
    fn format_current_session_marked() {
        let sessions = vec![test_meta("abcdef1234567890", Some("current"), 3)];
        let output = format_session_list(&sessions, Some("abcdef1234567890"));
        let raw = output;
        // Current session should have the cyan marker
        assert!(raw.contains("▸"));
    }

    #[test]
    fn format_unnamed_session() {
        let sessions = vec![test_meta("xyz1234567890", None, 1)];
        let output = format_session_list(&sessions, None);
        let plain = strip_ansi(&output);
        assert!(plain.contains("unnamed"));
    }

    #[test]
    fn format_multiple_sessions() {
        let sessions = vec![
            test_meta("aaa1234567890", Some("first"), 10),
            test_meta("bbb1234567890", Some("second"), 3),
        ];
        let output = format_session_list(&sessions, None);
        let plain = strip_ansi(&output);
        assert!(plain.contains("first"));
        assert!(plain.contains("second"));
    }

    #[test]
    fn format_long_working_dir_truncated() {
        let mut meta = test_meta("long1234567890", Some("test"), 1);
        meta.working_dir = "/very/long/path/that/exceeds/forty/characters/here".to_string();
        let sessions = vec![meta];
        let output = format_session_list(&sessions, None);
        let plain = strip_ansi(&output);
        assert!(plain.contains("..."));
    }
}
