use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use uuid::Uuid;

use serde::{Deserialize, Serialize};

use crate::{mode::AgentMode, provider::Message};

// ── Data types ──────────────────────────────────────────────────────────────

/// Metadata about a session, stored as the first line of the JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier (UUID v4).
    pub id: String,
    /// Working directory when the session was created.
    pub working_dir: String,
    /// Unix timestamp (seconds) when the session was created.
    pub created_at: u64,
    /// Unix timestamp (seconds) when the session was last updated.
    pub updated_at: u64,
    /// The agent mode used in this session.
    pub mode: AgentMode,
    /// The provider kind (e.g. "ollama", "llama.cpp", "vllm").
    pub provider: String,
    /// The model name.
    pub model: Option<String>,
    /// Optional user-defined name for the session.
    pub name: Option<String>,
    /// Number of messages in the session.
    pub message_count: usize,
}

/// A single line in the session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    /// First entry: session metadata.
    #[serde(rename = "meta")]
    Meta(SessionMeta),
    /// A conversation message.
    #[serde(rename = "message")]
    Message(Message),
}

// ── Session directory ──────────────────────────────────────────────────────

fn sessions_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/tinyharness/sessions")
}

fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Session handle ─────────────────────────────────────────────────────────

/// Manages a session's lifecycle: writing entries to the JSONL file.
pub struct Session {
    meta: SessionMeta,
    path: PathBuf,
    dirty: bool,                // whether we need to rewrite the meta line
    messages_since_save: usize, // counter for auto-save threshold
    created: bool,              // is on disk
}

/// Auto-save threshold: flush metadata every N messages appended.
const AUTO_SAVE_INTERVAL: usize = 5;

impl Session {
    /// Create a brand-new session. The session file is NOT written to disk
    /// until the first message is appended (lazy creation), so that
    /// command-only sessions don't leave empty files behind.
    pub fn new(working_dir: &str, mode: AgentMode, provider: &str, model: Option<String>) -> Self {
        let id = Uuid::new_v4().to_string();
        let now = now_timestamp();
        // Auto-generate a session name from the working directory basename
        let auto_name = std::path::Path::new(working_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("session")
            .to_string();

        let meta = SessionMeta {
            id,
            working_dir: working_dir.to_string(),
            created_at: now,
            updated_at: now,
            mode,
            provider: provider.to_string(),
            model,
            name: Some(auto_name),
            message_count: 0,
        };

        let dir = sessions_dir();
        fs::create_dir_all(&dir).ok();

        let path = dir.join(format!("{}.jsonl", meta.id));

        Session {
            meta,
            path,
            dirty: true,
            messages_since_save: 0,
            created: false,
        }
    }

    /// Resume an existing session from its JSONL file.
    pub fn load(session_id: &str) -> Result<(Self, Vec<Message>), String> {
        let dir = sessions_dir();
        let path = dir.join(format!("{}.jsonl", session_id));

        if !path.exists() {
            return Err(format!("Session '{}' not found", session_id));
        }

        let file = fs::File::open(&path).map_err(|e| format!("Failed to open session: {}", e))?;
        let reader = io::BufReader::new(file);

        let mut meta: Option<SessionMeta> = None;
        let mut messages: Vec<Message> = Vec::new();

        for line in reader.lines() {
            let line = line.map_err(|e| format!("Failed to read session line: {}", e))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEntry>(trimmed) {
                Ok(SessionEntry::Meta(m)) => {
                    meta = Some(m);
                }
                Ok(SessionEntry::Message(msg)) => {
                    messages.push(msg);
                }
                Err(e) => {
                    // Skip malformed lines rather than failing
                    eprintln!("Warning: Skipping malformed session entry: {}", e);
                }
            }
        }

        let meta = meta.ok_or_else(|| "Session file has no metadata entry".to_string())?;

        let session = Session {
            meta: meta.clone(),
            path,
            dirty: false,
            messages_since_save: 0,
            created: true,
        };

        Ok((session, messages))
    }

    /// Find the most recent session for a given working directory.
    pub fn find_latest_for_dir(working_dir: &str) -> Option<String> {
        let dir = sessions_dir();
        if !dir.exists() {
            return None;
        }

        let entries = fs::read_dir(&dir).ok()?;
        let mut best: Option<(u64, String)> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            // Peek at the first line to get metadata
            let file = fs::File::open(&path).ok()?;
            let mut reader = io::BufReader::new(file);
            let mut first_line = String::new();
            if reader.read_line(&mut first_line).ok()? == 0 {
                continue;
            }

            if let Ok(SessionEntry::Meta(meta)) =
                serde_json::from_str::<SessionEntry>(first_line.trim())
            {
                if meta.working_dir == working_dir {
                    match &mut best {
                        Some((best_time, best_id)) => {
                            if meta.updated_at > *best_time {
                                *best_time = meta.updated_at;
                                *best_id = meta.id.clone();
                            }
                        }
                        None => {
                            best = Some((meta.updated_at, meta.id.clone()));
                        }
                    }
                }
            }
        }

        best.map(|(_, id)| id)
    }

    /// Find a session by an ID prefix (e.g. first 12 chars).
    /// Returns the full session ID if exactly one match is found.
    pub fn find_by_prefix(prefix: &str) -> Result<String, String> {
        let all = Self::list_all();
        let matches: Vec<&SessionMeta> = all.iter().filter(|m| m.id.starts_with(prefix)).collect();

        match matches.len() {
            0 => Err(format!("No session found matching '{}'", prefix)),
            1 => Ok(matches[0].id.clone()),
            _ => {
                let ids: Vec<&str> = matches
                    .iter()
                    .map(|m| &m.id[..12.min(m.id.len())])
                    .collect();
                Err(format!(
                    "Multiple sessions match '{}': {}",
                    prefix,
                    ids.join(", ")
                ))
            }
        }
    }

    /// List all sessions, most recently updated first.
    pub fn list_all() -> Vec<SessionMeta> {
        let dir = sessions_dir();
        if !dir.exists() {
            return Vec::new();
        }

        let entries = fs::read_dir(&dir).ok();
        let mut sessions: Vec<SessionMeta> = Vec::new();

        if let Some(entries) = entries {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                let file = match fs::File::open(&path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let mut reader = io::BufReader::new(file);
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).ok().unwrap_or(0) == 0 {
                    continue;
                }

                if let Ok(SessionEntry::Meta(meta)) =
                    serde_json::from_str::<SessionEntry>(first_line.trim())
                {
                    sessions.push(meta);
                }
            }
        }

        // Sort by updated_at descending (most recent first)
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }

    // ── Mutating operations ────────────────────────────────────────────

    /// Append a message to the session file and update metadata.
    /// Triggers an auto-save (flush) every [`AUTO_SAVE_INTERVAL`] messages.
    /// On the first call, materializes the session file on disk.
    pub fn append_message(&mut self, message: &Message) {
        self.ensure_created();
        self.write_entry(&SessionEntry::Message(message.clone()));
        self.meta.message_count += 1;
        self.meta.updated_at = now_timestamp();
        self.dirty = true;
        self.messages_since_save += 1;
        if self.messages_since_save >= AUTO_SAVE_INTERVAL {
            self.flush();
            self.messages_since_save = 0;
        }
    }

    /// Update the session mode.
    pub fn set_mode(&mut self, mode: AgentMode) {
        self.meta.mode = mode;
        self.meta.updated_at = now_timestamp();
        self.dirty = true;
    }

    /// Update the model name.
    pub fn set_model(&mut self, model: Option<String>) {
        self.meta.model = model;
        self.meta.updated_at = now_timestamp();
        self.dirty = true;
    }

    /// Set a human-readable name for the session.
    pub fn set_name(&mut self, name: String) {
        self.meta.name = Some(name);
        self.meta.updated_at = now_timestamp();
        self.dirty = true;
    }

    /// Flush any metadata changes back to the file (rewrite the first line).
    /// If the session file hasn't been created yet, materializes it first.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }

        self.ensure_created();

        // Rewrite the file: read all lines, replace the first line with updated meta,
        // write everything back atomically.
        let file = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Warning: Failed to read session file for flush: {}", e);
                return;
            }
        };

        let lines: Vec<&str> = file.lines().collect();
        if lines.is_empty() {
            return;
        }

        let meta_line = match serde_json::to_string(&SessionEntry::Meta(self.meta.clone())) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Warning: Failed to serialize session meta: {}", e);
                return;
            }
        };

        let tmp_path = self.path.with_extension("jsonl.tmp");

        match fs::File::create(&tmp_path) {
            Ok(mut file) => {
                // Write updated meta as first line
                if write!(file, "{}\n", meta_line).is_err() {
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
                // Write remaining lines (skip original first line)
                for line in lines.iter().skip(1) {
                    if write!(file, "{}\n", line).is_err() {
                        let _ = fs::remove_file(&tmp_path);
                        return;
                    }
                }
                if file.flush().is_err() {
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create temp session file: {}", e);
                return;
            }
        }

        if fs::rename(&tmp_path, &self.path).is_err() {
            let _ = fs::remove_file(&tmp_path);
        }

        self.dirty = false;
    }

    // ── Accessors ──────────────────────────────────────────────────────

    pub fn id(&self) -> &str {
        &self.meta.id
    }

    pub fn meta(&self) -> &SessionMeta {
        &self.meta
    }

    // ── Internal ────────────────────────────────────────────────────────

    /// Materialize the session file on disk if it hasn't been created yet.
    /// Writes the meta entry as the first line.
    fn ensure_created(&mut self) {
        if self.created {
            return;
        }
        self.write_entry(&SessionEntry::Meta(self.meta.clone()));
        self.created = true;
    }

    fn write_entry(&self, entry: &SessionEntry) {
        let line = match serde_json::to_string(entry) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Warning: Failed to serialize session entry: {}", e);
                return;
            }
        };

        match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                if let Err(e) = writeln!(file, "{}", line) {
                    eprintln!("Warning: Failed to write session entry: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to open session file for writing: {}", e);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Only flush if the file was actually created — don't materialize
        // a lazy session just because it was dropped.
        if self.created {
            self.flush();
        }
    }
}

/// Format a session for display in the `/sessions` listing.
pub fn format_session_list(sessions: &[SessionMeta], current_id: Option<&str>) -> String {
    use crate::style::*;

    let mut output = String::new();

    if sessions.is_empty() {
        output.push_str(&format!("{}No sessions found.{}", ORANGE, RESET));
        return output;
    }

    output.push_str(&format!(
        "{}Available sessions (most recent first):{}\n\n",
        BOLD, RESET
    ));

    for meta in sessions {
        let is_current = current_id == Some(meta.id.as_str());
        let marker = if is_current {
            format!("{}▸{}", CYAN, RESET)
        } else {
            " ".to_string()
        };

        let age = format_age(now_timestamp().saturating_sub(meta.updated_at));
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

/// Format a duration in seconds as a human-friendly age string.
fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}
