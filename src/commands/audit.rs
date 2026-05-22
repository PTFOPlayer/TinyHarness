use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::style::*;

// ── Data types ──────────────────────────────────────────────────────────────

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix timestamp (seconds) when the command was executed.
    pub timestamp: u64,
    /// Session ID where the command was run.
    pub session_id: String,
    /// The tool that was executed (e.g. "run", "write", "edit").
    #[serde(default = "default_tool_name")]
    pub tool_name: String,
    /// The primary argument: shell command for "run", file path for "write"/"edit".
    pub command: String,
    /// Exit code of the command (0 = success).
    pub exit_code: i32,
    /// Whether the command was auto-accepted (true) or user-confirmed (false).
    pub auto_accepted: bool,
    /// Duration of the command execution in milliseconds.
    pub duration_ms: u64,
}

fn default_tool_name() -> String {
    "run".to_string()
}

// ── Audit log path ──────────────────────────────────────────────────────────

/// Get the default audit log path: ~/.local/share/tinyharness/audit.jsonl
pub fn audit_log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/tinyharness/audit.jsonl")
}

/// Ensure the audit log directory exists.
pub fn ensure_audit_dir() -> std::io::Result<()> {
    let path = audit_log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

// ── Append to audit log ─────────────────────────────────────────────────────

/// Append a command execution to the audit log.
pub fn log_command(
    session_id: &str,
    tool_name: &str,
    command: &str,
    exit_code: i32,
    auto_accepted: bool,
    duration_ms: u64,
) {
    let _ = ensure_audit_dir();

    let entry = AuditEntry {
        timestamp: now_timestamp(),
        session_id: session_id.to_string(),
        tool_name: tool_name.to_string(),
        command: command.to_string(),
        exit_code,
        auto_accepted,
        duration_ms,
    };

    let line = match serde_json::to_string(&entry) {
        Ok(l) => l,
        Err(_) => return,
    };

    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_log_path())
    {
        let _ = writeln!(file, "{}", line);
    }
}

// ── Read audit log ──────────────────────────────────────────────────────────

/// Read the last N entries from the audit log.
pub fn read_last(n: usize) -> Vec<AuditEntry> {
    let path = audit_log_path();
    if !path.exists() {
        return Vec::new();
    }

    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = io::BufReader::new(file);
    let mut entries: Vec<AuditEntry> = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        if let Ok(entry) = serde_json::from_str::<AuditEntry>(&line) {
            entries.push(entry);
        }
    }

    // Return last N entries (most recent first)
    entries.into_iter().rev().take(n).collect()
}

/// Read entries for a specific session.
pub fn read_session(session_id: &str) -> Vec<AuditEntry> {
    let all = read_last(1000); // Load recent entries
    all.into_iter()
        .filter(|e| e.session_id.starts_with(session_id))
        .collect()
}

/// Clear the audit log.
pub fn clear() -> std::io::Result<()> {
    let path = audit_log_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

// ── Display ─────────────────────────────────────────────────────────────────

/// Format a timestamp as a human-readable string.
fn format_timestamp(ts: u64) -> String {
    let duration = std::time::Duration::from_secs(ts);
    let datetime = UNIX_EPOCH + duration;

    // Format as YYYY-MM-DD HH:MM:SS
    let system_time: SystemTime = datetime;
    let datetime: chrono::DateTime<chrono::Local> = system_time.into();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Display the last N audit entries in a table.
pub fn show_last(n: usize) {
    let entries = read_last(n);

    if entries.is_empty() {
        println!("{}No audit entries found.{}", ORANGE, RESET);
        return;
    }

    println!("\n{}Recent command audit (last {}):{}\n", BOLD, n, RESET);

    // Header
    println!(
        "  {}{:20}  {:6}  {:26}  {:6}  {:8}  {:10}{}",
        BOLD, "Timestamp", "Tool", "Command", "Exit", "Auto?", "Duration", RESET
    );
    println!(
        "  {}{:20}  {:6}  {:26}  {:6}  {:8}  {:10}{}",
        GRAY,
        "────────────────────",
        "──────",
        "──────────────────────────",
        "──────",
        "────────",
        "──────────",
        RESET
    );

    for entry in &entries {
        let ts_str = format_timestamp(entry.timestamp);
        let tool_display = if entry.tool_name.len() > 6 {
            format!("{}...", &entry.tool_name[..3])
        } else {
            entry.tool_name.clone()
        };
        let cmd_display = if entry.command.len() > 26 {
            format!("{}...", &entry.command[..23])
        } else {
            entry.command.clone()
        };

        let exit_str = if entry.exit_code == 0 {
            format!("{}{:6}{}", GREEN, entry.exit_code, RESET)
        } else {
            format!("{}{:6}{}", RED, entry.exit_code, RESET)
        };

        let auto_str = if entry.auto_accepted {
            format!("{}✓ yes{}   ", GREEN, RESET)
        } else {
            format!("{}✗ no{}    ", ORANGE, RESET)
        };

        let duration_str = if entry.duration_ms >= 1000 {
            format!(
                "{}{:.1}s{}    ",
                BLUE,
                entry.duration_ms as f64 / 1000.0,
                RESET
            )
        } else {
            format!("{}{}ms{}     ", GRAY, entry.duration_ms, RESET)
        };

        println!(
            "  {}{}  {}{}{}  {}  {}  {}  {}",
            GRAY, ts_str, CYAN, tool_display, RESET, cmd_display, exit_str, auto_str, duration_str
        );
    }

    println!();
}

/// Display audit entries for a specific session.
pub fn show_session(session_id: &str) {
    let entries = read_session(session_id);

    if entries.is_empty() {
        println!(
            "{}No audit entries found for session '{}'.{}",
            ORANGE, session_id, RESET
        );
        return;
    }

    println!("\n{}Audit for session '{}':{}\n", BOLD, session_id, RESET);

    // Header
    println!(
        "  {}{:20}  {:6}  {:26}  {:6}  {:8}  {:10}{}",
        BOLD, "Timestamp", "Tool", "Command", "Exit", "Auto?", "Duration", RESET
    );
    println!(
        "  {}{:20}  {:6}  {:26}  {:6}  {:8}  {:10}{}",
        GRAY,
        "────────────────────",
        "──────",
        "──────────────────────────",
        "──────",
        "────────",
        "──────────",
        RESET
    );

    for entry in &entries {
        let ts_str = format_timestamp(entry.timestamp);
        let tool_display = if entry.tool_name.len() > 6 {
            format!("{}...", &entry.tool_name[..3])
        } else {
            entry.tool_name.clone()
        };
        let cmd_display = if entry.command.len() > 26 {
            format!("{}...", &entry.command[..23])
        } else {
            entry.command.clone()
        };

        let exit_str = if entry.exit_code == 0 {
            format!("{}{:6}{}", GREEN, entry.exit_code, RESET)
        } else {
            format!("{}{:6}{}", RED, entry.exit_code, RESET)
        };

        let auto_str = if entry.auto_accepted {
            format!("{}✓ yes{}   ", GREEN, RESET)
        } else {
            format!("{}✗ no{}    ", ORANGE, RESET)
        };

        let duration_str = if entry.duration_ms >= 1000 {
            format!(
                "{}{:.1}s{}    ",
                BLUE,
                entry.duration_ms as f64 / 1000.0,
                RESET
            )
        } else {
            format!("{}{}ms{}     ", GRAY, entry.duration_ms, RESET)
        };

        println!(
            "  {}{}  {}{}{}  {}  {}  {}  {}",
            GRAY, ts_str, CYAN, tool_display, RESET, cmd_display, exit_str, auto_str, duration_str
        );
    }

    println!();
}

/// Execute the /audit command.
pub fn execute(args: &str) {
    let args = args.trim();

    if args.is_empty() {
        // Default: show last 20
        show_last(20);
        return;
    }

    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    match parts[0].to_lowercase().as_str() {
        "last" => {
            let n = parts
                .get(1)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20);
            show_last(n);
        }
        "session" => {
            let session_id = parts.get(1).unwrap_or(&"");
            if session_id.is_empty() {
                println!("{}Usage: /audit session <id>{}", ORANGE, RESET);
                return;
            }
            show_session(session_id);
        }
        "clear" => {
            println!(
                "{}⚠ This will delete the entire audit log. Type 'yes' to confirm: {}",
                ORANGE, RESET
            );
            print!("{}> {}", BOLD, RESET);
            io::stdout().flush().unwrap();

            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_ok() && input.trim().to_lowercase() == "yes" {
                match clear() {
                    Ok(()) => println!("{}✓ Audit log cleared.{}", GREEN, RESET),
                    Err(e) => println!("{}✗ Failed to clear audit log: {}{}", RED, e, RESET),
                }
            } else {
                println!("{}✗ Cancelled.{}", GRAY, RESET);
            }
        }
        _ => {
            println!("{}Usage:{}", ORANGE, RESET);
            println!("  /audit              - Show last 20 commands");
            println!("  /audit last <n>     - Show last N commands");
            println!("  /audit session <id> - Show commands from a session");
            println!("  /audit clear        - Delete the audit log");
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        let now = now_timestamp();
        let formatted = format_timestamp(now);
        assert!(formatted.contains('-'), "timestamp should contain dashes");
        assert!(formatted.contains(':'), "timestamp should contain colons");
    }
}
