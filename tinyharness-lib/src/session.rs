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

// ── Error type ──────────────────────────────────────────────────────────────

/// Error type for session I/O operations.
#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    NotFound(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "I/O error: {}", e),
            SessionError::Parse(e) => write!(f, "parse error: {}", e),
            SessionError::NotFound(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(e) => Some(e),
            SessionError::Parse(e) => Some(e),
            SessionError::NotFound(_) => None,
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        SessionError::Io(e)
    }
}

impl From<serde_json::Error> for SessionError {
    fn from(e: serde_json::Error) -> Self {
        SessionError::Parse(e)
    }
}

// ── Session store ───────────────────────────────────────────────────────────

/// Handles persistence of sessions as JSONL files in a directory.
///
/// By default, uses `~/.local/share/tinyharness/sessions/`. This can be
/// overridden with [`SessionStore::new`] for testing or alternative paths.
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    /// Create a store that uses the default session directory
    /// (`~/.local/share/tinyharness/sessions/`).
    pub fn default_path() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir = PathBuf::from(home).join(".local/share/tinyharness/sessions");
        SessionStore { dir }
    }

    /// Create a store that uses a custom directory.
    pub fn new(dir: PathBuf) -> Self {
        SessionStore { dir }
    }

    /// Ensure the session directory exists.
    pub fn ensure_dir(&self) -> Result<(), SessionError> {
        fs::create_dir_all(&self.dir)?;
        Ok(())
    }

    /// Create a brand-new session and write its initial metadata to disk.
    /// Returns the new `Session` handle.
    pub fn create(
        &self,
        working_dir: &str,
        mode: AgentMode,
        provider: &str,
        model: Option<String>,
    ) -> Session {
        let id = Uuid::new_v4().to_string();
        let now = now_timestamp();
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

        // Eagerly create the session file so callers get immediate feedback on errors
        let mut session = Session {
            meta: meta.clone(),
            path: self.dir.join(format!("{}.jsonl", meta.id)),
            dirty: true,
            messages_since_save: 0,
            created: false,
        };
        session.ensure_created();
        session
    }

    /// Load an existing session by its full ID.
    /// Returns the session handle and the conversation messages.
    pub fn load(&self, session_id: &str) -> Result<(Session, Vec<Message>), SessionError> {
        let path = self.dir.join(format!("{}.jsonl", session_id));

        if !path.exists() {
            return Err(SessionError::NotFound(format!(
                "Session '{}' not found",
                session_id
            )));
        }

        let file = fs::File::open(&path)?;
        let reader = io::BufReader::new(file);

        let mut meta: Option<SessionMeta> = None;
        let mut messages: Vec<Message> = Vec::new();

        for line in reader.lines() {
            let line = line?;
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
                Err(_) => {
                    // Skip malformed lines rather than failing
                }
            }
        }

        let meta = meta.ok_or_else(|| {
            SessionError::Parse(serde_json::from_str::<SessionEntry>("").unwrap_err())
        })?;

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
    pub fn find_latest_for_dir(&self, working_dir: &str) -> Option<String> {
        if !self.dir.exists() {
            return None;
        }

        let entries = fs::read_dir(&self.dir).ok()?;
        let mut best: Option<(u64, String)> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            if let Some(meta) = read_session_meta(&path)
                && meta.working_dir == working_dir
            {
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

        best.map(|(_, id)| id)
    }

    /// Find a session by an ID prefix.
    pub fn find_by_prefix(&self, prefix: &str) -> Result<String, SessionError> {
        let all = self.list_all();
        let matches: Vec<&SessionMeta> = all.iter().filter(|m| m.id.starts_with(prefix)).collect();

        match matches.len() {
            0 => Err(SessionError::NotFound(format!(
                "No session found matching '{}'",
                prefix
            ))),
            1 => Ok(matches[0].id.clone()),
            _ => {
                let ids: Vec<&str> = matches
                    .iter()
                    .map(|m| &m.id[..12.min(m.id.len())])
                    .collect();
                Err(SessionError::NotFound(format!(
                    "Multiple sessions match '{}': {}",
                    prefix,
                    ids.join(", ")
                )))
            }
        }
    }

    /// List all sessions, most recently updated first.
    pub fn list_all(&self) -> Vec<SessionMeta> {
        if !self.dir.exists() {
            return Vec::new();
        }

        let mut sessions: Vec<SessionMeta> = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                if let Some(meta) = read_session_meta(&path) {
                    sessions.push(meta);
                }
            }
        }

        sessions.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        sessions
    }

    /// Returns the directory path where sessions are stored.
    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    /// Delete a session by its ID.
    pub fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        let path = self.dir.join(format!("{}.jsonl", session_id));
        
        if !path.exists() {
            return Err(SessionError::NotFound(format!(
                "Session '{}' not found",
                session_id
            )));
        }

        fs::remove_file(&path)?;
        Ok(())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_session_meta(path: &PathBuf) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let mut reader = io::BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).ok()? == 0 {
        return None;
    }
    if let Ok(SessionEntry::Meta(meta)) = serde_json::from_str::<SessionEntry>(first_line.trim()) {
        Some(meta)
    } else {
        None
    }
}

// ── Session handle ─────────────────────────────────────────────────────────

/// Manages a session's lifecycle: writing entries to the JSONL file.
///
/// Use [`SessionStore`] to create or load sessions. The `Session` type
/// handles the actual message appending, metadata updates, and auto-save.
pub struct Session {
    meta: SessionMeta,
    path: PathBuf,
    dirty: bool,
    messages_since_save: usize,
    created: bool,
}

/// Auto-save threshold: flush metadata every N messages appended.
const AUTO_SAVE_INTERVAL: usize = 5;

impl Session {
    // ── Mutating operations ────────────────────────────────────────────

    /// Append a message to the session file and update metadata.
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
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }

        self.ensure_created();

        let file = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(_) => return,
        };

        let lines: Vec<&str> = file.lines().collect();
        if lines.is_empty() {
            return;
        }

        let meta_line = match serde_json::to_string(&SessionEntry::Meta(self.meta.clone())) {
            Ok(l) => l,
            Err(_) => return,
        };

        let tmp_path = self.path.with_extension("jsonl.tmp");

        match fs::File::create(&tmp_path) {
            Ok(mut file) => {
                if writeln!(file, "{}", meta_line).is_err() {
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
                for line in lines.iter().skip(1) {
                    if writeln!(file, "{}", line).is_err() {
                        let _ = fs::remove_file(&tmp_path);
                        return;
                    }
                }
                if file.flush().is_err() {
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
            }
            Err(_) => return,
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

    fn ensure_created(&mut self) {
        if self.created {
            return;
        }

        // Ensure directory exists
        let _ = fs::create_dir_all(
            self.path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
        );

        self.write_entry(&SessionEntry::Meta(self.meta.clone()));
        self.created = true;
    }

    fn write_entry(&self, entry: &SessionEntry) {
        let line = match serde_json::to_string(entry) {
            Ok(l) => l,
            Err(_) => return,
        };

        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{}", line);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.created {
            self.flush();
        }
    }
}

/// Format a duration in seconds as a human-friendly age string.
pub fn format_age(secs: u64) -> String {
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
