use crate::provider::{Message, Role};
use crate::style::*;

/// Manages pinned files whose content is injected into the system prompt context.
#[derive(Debug, Clone)]
pub struct FileContext {
    /// Ordered list of (path, content) pairs for pinned files.
    pinned_files: Vec<(String, String)>,
}

impl FileContext {
    pub fn new() -> Self {
        FileContext {
            pinned_files: Vec::new(),
        }
    }

    /// Add a file to the pinned context. Returns an error message if the file cannot be read.
    pub fn add(&mut self, path: &str) -> Result<String, String> {
        // Check for duplicates
        if self.pinned_files.iter().any(|(p, _)| p == path) {
            return Err(format!("File '{}' is already pinned.", path));
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read '{}': {}", path, e))?;

        let line_count = content.lines().count();
        let size = content.len();
        self.pinned_files.push((path.to_string(), content));

        Ok(format!(
            "Pinned '{}' ({} lines, {} bytes). Total pinned files: {}",
            path,
            line_count,
            size,
            self.pinned_files.len()
        ))
    }

    /// Remove a file from the pinned context.
    pub fn drop(&mut self, path: &str) -> Result<String, String> {
        let original_len = self.pinned_files.len();
        self.pinned_files.retain(|(p, _)| p != path);

        if self.pinned_files.len() == original_len {
            // Maybe it was a partial match — try basename
            let path_basename = std::path::Path::new(path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string());

            if let Some(basename) = path_basename {
                if let Some(idx) = self
                    .pinned_files
                    .iter()
                    .position(|(p, _)| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()) == Some(basename.clone()))
                {
                    self.pinned_files.remove(idx);
                    return Ok(format!("Unpinned '{}' (matched by basename). Total pinned files: {}", path, self.pinned_files.len()));
                }
            }

            return Err(format!(
                "File '{}' not found in pinned context. Use /files to see pinned files.",
                path
            ));
        }

        Ok(format!(
            "Unpinned '{}'. Total pinned files: {}",
            path,
            self.pinned_files.len()
        ))
    }

    /// Clear all pinned files.
    pub fn clear(&mut self) -> String {
        let count = self.pinned_files.len();
        self.pinned_files.clear();
        format!("Cleared all pinned files (was {}).", count)
    }

    /// List all pinned files with summary info.
    pub fn list(&self) -> String {
        if self.pinned_files.is_empty() {
            return "No files pinned. Use /add <path> to pin a file.".to_string();
        }

        let mut result = String::new();
        result.push_str(&format!("{}Pinned files ({}):\n", BOLD, self.pinned_files.len()));
        for (path, content) in &self.pinned_files {
            let lines = content.lines().count();
            let size = content.len();
            result.push_str(&format!(
                "  {}{}{} ({} lines, {} bytes)\n",
                BLUE, path, RESET, lines, size
            ));
        }
        result
    }

    /// Check if any files are pinned.
    pub fn is_empty(&self) -> bool {
        self.pinned_files.is_empty()
    }

    /// Get the number of pinned files.
    pub fn pinned_file_count(&self) -> usize {
        self.pinned_files.len()
    }

    /// Refresh all pinned files (re-read from disk).
    pub fn refresh(&mut self) -> String {
        let mut errors = Vec::new();
        let mut refreshed = 0;

        for (path, content) in &mut self.pinned_files {
            match std::fs::read_to_string(path.as_str()) {
                Ok(new_content) => {
                    *content = new_content;
                    refreshed += 1;
                }
                Err(e) => {
                    errors.push(format!("Failed to refresh '{}': {}", path, e));
                }
            }
        }

        let mut result = format!(
            "{}Refreshed {}/{} pinned files.{}",
            GREEN, refreshed, self.pinned_files.len(), RESET
        );
        if !errors.is_empty() {
            result.push_str(&format!("\n{}Errors:\n{}{}", RED, errors.join("\n"), RESET));
        }
        result
    }

    /// Format the pinned files context for injection into the system prompt.
    pub fn format_for_prompt(&self) -> String {
        if self.pinned_files.is_empty() {
            return String::new();
        }

        let mut sections = Vec::new();
        sections.push(format!(
            "\nThe following files are pinned in context (available without reading):"
        ));

        for (path, content) in &self.pinned_files {
            let line_count = content.lines().count();
            sections.push(format!("\n--- {} ({} lines) ---\n{}", path, line_count, content));
        }

        sections.push("\n--- End of pinned files ---\nUse these files as reference when answering questions. You don't need to read them again.".to_string());

        sections.join("\n")
    }
}

/// Handle the /add command.
pub fn execute_add(file_context: &mut FileContext, path: &str) {
    match file_context.add(path) {
        Ok(msg) => println!("{}{}{}", GREEN, msg, RESET),
        Err(e) => println!("{}{}{}", RED, e, RESET),
    }
}

/// Handle the /drop command.
pub fn execute_drop(file_context: &mut FileContext, path: &str) {
    match file_context.drop(path) {
        Ok(msg) => println!("{}{}{}", GREEN, msg, RESET),
        Err(e) => println!("{}{}{}", RED, e, RESET),
    }
}

/// Handle the /files command.
pub fn execute_list(file_context: &FileContext) {
    println!("\n{}", file_context.list());
}

/// Handle the /dropall command.
pub fn execute_clear(file_context: &mut FileContext) {
    println!("{}{}{}", GREEN, file_context.clear(), RESET);
}

/// Handle the /refresh command (re-read pinned files from disk).
pub fn execute_refresh(file_context: &mut FileContext) {
    println!("{}", file_context.refresh());
}

/// Inject pinned file content into the system prompt message.
/// This modifies the system prompt in-place to include pinned file content.
pub fn inject_into_system_prompt(messages: &mut Vec<Message>, file_context: &FileContext) {
    if file_context.is_empty() {
        return;
    }

    let pinned_content = file_context.format_for_prompt();

    // Find and update the system message
    if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == Role::System) {
        // Check if we already have pinned content and replace it, or append
        if let Some(start_idx) = sys_msg.content.find("\nThe following files are pinned in context") {
            // Replace existing pinned content
            sys_msg.content.truncate(start_idx);
            sys_msg.content.push_str(&pinned_content);
        } else {
            // Append pinned content
            sys_msg.content.push_str(&pinned_content);
        }
    }
}

/// Remove pinned file content from the system prompt (before rebuilding it).
/// Returns the system prompt without pinned content.
pub fn strip_pinned_content(system_prompt: &str) -> String {
    if let Some(idx) = system_prompt.find("\nThe following files are pinned in context") {
        system_prompt[..idx].to_string()
    } else {
        system_prompt.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: create a temp file and return its path.
    fn create_temp_file(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{}", content).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_add_file() {
        let f = create_temp_file("fn main() {}");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        let result = ctx.add(path).unwrap();
        assert!(result.contains("Pinned"));
        assert!(result.contains("1 lines"));
        assert_eq!(ctx.pinned_file_count(), 1);
    }

    #[test]
    fn test_add_duplicate() {
        let f = create_temp_file("hello");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();
        let result = ctx.add(path);
        assert!(result.is_err());
        assert_eq!(ctx.pinned_file_count(), 1);
    }

    #[test]
    fn test_drop_file() {
        let f = create_temp_file("hello");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();
        let result = ctx.drop(path).unwrap();
        assert!(result.contains("Unpinned"));
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_drop_nonexistent() {
        let mut ctx = FileContext::new();
        let result = ctx.drop("/nonexistent/file.rs");
        assert!(result.is_err());
    }

    #[test]
    fn test_clear() {
        let f1 = create_temp_file("file1");
        let f2 = create_temp_file("file2");
        let mut ctx = FileContext::new();
        ctx.add(f1.path().to_str().unwrap()).unwrap();
        ctx.add(f2.path().to_str().unwrap()).unwrap();
        assert_eq!(ctx.pinned_file_count(), 2);
        let result = ctx.clear();
        assert!(result.contains("Cleared"));
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_format_for_prompt() {
        let f = create_temp_file("fn main() {}");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();
        let formatted = ctx.format_for_prompt();
        assert!(formatted.contains("pinned in context"));
        assert!(formatted.contains(path));
        assert!(formatted.contains("fn main()"));
    }

    #[test]
    fn test_format_empty() {
        let ctx = FileContext::new();
        assert!(ctx.format_for_prompt().is_empty());
    }

    #[test]
    fn test_list_empty() {
        let ctx = FileContext::new();
        assert!(ctx.list().contains("No files pinned"));
    }

    #[test]
    fn test_list_with_files() {
        let f = create_temp_file("hello world");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();
        let list = ctx.list();
        assert!(list.contains("Pinned files (1)"));
        assert!(list.contains(path));
    }

    #[test]
    fn test_inject_into_system_prompt() {
        let f = create_temp_file("use crate::foo;");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();

        let mut messages = vec![Message {
            role: Role::System,
            content: "You are a helpful assistant.".to_string(),
            tool_calls: vec![],
        }];

        inject_into_system_prompt(&mut messages, &ctx);
        assert!(messages[0].content.contains("pinned in context"));
        assert!(messages[0].content.contains("use crate::foo"));
    }

    #[test]
    fn test_inject_replaces_existing() {
        let f = create_temp_file("updated content");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();

        let mut messages = vec![Message {
            role: Role::System,
            content: "Base prompt\n\nThe following files are pinned in context\n--- old ---\nold content\n--- End of pinned files ---".to_string(),
            tool_calls: vec![],
        }];

        inject_into_system_prompt(&mut messages, &ctx);
        assert!(messages[0].content.contains("updated content"));
        assert!(!messages[0].content.contains("old content"));
        assert!(messages[0].content.contains("Base prompt"));
    }

    #[test]
    fn test_strip_pinned_content() {
        let prompt = "Base instructions\n\nThe following files are pinned in context\n--- file.rs ---\nhello\n--- End of pinned files ---";
        let stripped = strip_pinned_content(prompt);
        assert_eq!(stripped.trim(), "Base instructions");
        assert!(!stripped.contains("pinned"));
    }

    #[test]
    fn test_strip_no_pinned() {
        let prompt = "Just a normal system prompt";
        let stripped = strip_pinned_content(prompt);
        assert_eq!(stripped, "Just a normal system prompt");
    }

    #[test]
    fn test_add_nonexistent_file() {
        let mut ctx = FileContext::new();
        let result = ctx.add("/absolutely/does/not/exist.rs");
        assert!(result.is_err());
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_refresh_updates_content() {
        let f = create_temp_file("original content");
        let path = f.path().to_str().unwrap();
        let mut ctx = FileContext::new();
        ctx.add(path).unwrap();

        // Overwrite the file
        std::fs::write(path, "updated content").unwrap();
        ctx.refresh();

        let formatted = ctx.format_for_prompt();
        assert!(formatted.contains("updated content"));
        assert!(!formatted.contains("original content"));
    }
}