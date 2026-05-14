use tinyharness_lib::session::{SessionMeta, SessionStore, format_age};

use crate::style::*;
use std::io::{self, Write};

/// Format a session for display in the `/sessions` listing.
fn format_session_list(sessions: &[SessionMeta], current_id: Option<&str>) -> String {
    let mut output = String::new();

    if sessions.is_empty() {
        output.push_str(&format!("{}No sessions found.{}", ORANGE, RESET));
        return output;
    }

    output.push_str(&format!(
        "{}Available sessions (most recent first):{}\n\n",
        BOLD, RESET
    ));

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for meta in sessions {
        let is_current = current_id == Some(meta.id.as_str());
        let marker = if is_current {
            format!("{}▸{}", CYAN, RESET)
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
            "{} {}{}{} — {}{}{}\n",
            marker,
            BLUE,
            &meta.id[..12],
            RESET,
            BOLD,
            name_str,
            RESET,
        ));
        output.push_str(&format!(
            "  {}  {}{} msgs, {}{}{}  {}{}\n",
            marker, GRAY, meta.message_count, ITALIC, age, RESET, GRAY, dir_display,
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
                eprintln!(
                    "{}Warning: Failed to delete empty session {}: {}{}",
                    RED,
                    &meta.id[..8],
                    e,
                    RESET
                );
            } else {
                deleted += 1;
            }
        }
    }

    deleted
}

pub fn execute_list(current_session_id: Option<&str>) {
    let store = SessionStore::default_path();

    // Auto-delete empty sessions before listing
    let deleted = cleanup_empty_sessions();
    if deleted > 0 {
        println!("{}ℹ Cleaned up {} empty session(s){}", GRAY, deleted, RESET);
    }

    let sessions = store.list_all();
    let output = format_session_list(&sessions, current_session_id);
    println!("{}", output);
}

/// Delete a session by ID or name.
pub fn execute_delete(session_id: &str, current_session_id: Option<&str>) {
    let store = SessionStore::default_path();

    // Find the session
    let meta = store
        .list_all()
        .into_iter()
        .find(|s| s.id.starts_with(session_id) || s.name.as_ref().is_some_and(|n| n == session_id));

    let meta = match meta {
        Some(m) => m,
        None => {
            println!("{}✗ Session '{}' not found{}", RED, session_id, RESET);
            return;
        }
    };

    // Prevent deleting current session without warning
    let is_current = current_session_id == Some(meta.id.as_str());

    let name_str = meta.name.as_deref().unwrap_or("unnamed");
    println!(
        "{}⚠ Delete session \"{}\" ({} messages)? This cannot be undone.{}",
        ORANGE, name_str, meta.message_count, RESET
    );
    print!("{}Type 'yes' to confirm: {}", BOLD, RESET);
    io::stdout().flush().unwrap();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        println!("{}✗ Cancelled{}", RED, RESET);
        return;
    }

    if input.trim() != "yes" {
        println!("{}✗ Cancelled{}", GRAY, RESET);
        return;
    }

    if let Err(e) = store.delete(&meta.id) {
        println!("{}✗ Failed to delete session: {}{}", RED, e, RESET);
        return;
    }

    println!(
        "{}✓ Deleted session {} — \"{}\"{}",
        GREEN,
        &meta.id[..12],
        name_str,
        RESET
    );

    // If we deleted the current session, warn the user
    if is_current {
        println!(
            "{}⚠ You deleted the current session. Consider switching to another session or starting a new one.{}",
            ORANGE, RESET
        );
    }
}
