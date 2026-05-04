use std::{
    fs,
    path::{Path, PathBuf},
};

/// Maximum size for a project instruction file (characters).
/// Files exceeding this are truncated with a notice. Matches Hermes Agent's limit.
const PROJECT_MD_MAX_CHARS: usize = 20_000;

/// Head retention ratio for truncated files (70%).
const PROJECT_MD_HEAD_RATIO: f64 = 0.70;

/// File names to search for, in priority order (first match wins).
/// Mirrors the priority system used by Hermes Agent:
///   .hermes.md → AGENTS.md → CLAUDE.md → .cursorrules
pub const PROJECT_MD_FILE_NAMES: &[&str] = &[
    "TINYHARNESS.md",
    ".tinyharness.md",
    "AGENTS.md",
    "CLAUDE.md",
];

/// Collected metadata about the workspace/repository the agent is operating in.
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    /// Absolute path to the workspace root (current working directory).
    pub root: PathBuf,
    /// Detected project type (e.g. "Rust", "Node.js", "Python", "Unknown").
    pub project_type: String,
    /// Project name extracted from Cargo.toml / package.json / setup.py etc.
    pub project_name: String,
    /// Top-level directory listing (files and dirs, one level deep).
    pub structure: Vec<String>,
    /// Whether a .git directory exists.
    pub is_git_repo: bool,
    /// Detected build command (e.g. "cargo build", "npm run build").
    pub build_command: String,
    /// Detected test command.
    pub test_command: String,
    /// Contents of the discovered project instruction file (TINYHARNESS.md, AGENTS.md, etc.).
    /// `None` if no file was found.
    pub project_md: Option<(String, String)>, // (filename, content)
}

impl WorkspaceContext {
    /// Collect workspace context from the current working directory.
    pub fn collect() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_type = detect_project_type(&root);
        let project_name = detect_project_name(&root, &project_type);
        let structure = list_top_level(&root);
        let is_git_repo = root.join(".git").is_dir();
        let (build_command, test_command) = detect_commands(&project_type);
        let project_md = discover_project_md(&root);

        WorkspaceContext {
            root,
            project_type,
            project_name,
            structure,
            is_git_repo,
            build_command,
            test_command,
            project_md,
        }
    }

    /// Format the context as a human-readable string to inject into the system prompt.
    pub fn format(&self) -> String {
        let mut lines = Vec::new();

        lines.push(format!(
            "You are operating inside a {} project called \"{}\".",
            self.project_type, self.project_name
        ));
        lines.push(format!("Workspace root: {}", self.root.display()));

        if self.is_git_repo {
            lines.push("This is a git repository.".to_string());
        }

        lines.push("\nProject structure:".to_string());
        for entry in &self.structure {
            lines.push(format!("  {}", entry));
        }

        if !self.build_command.is_empty() {
            lines.push(format!("\nBuild command: {}", self.build_command));
        }
        if !self.test_command.is_empty() {
            lines.push(format!("Test command: {}", self.test_command));
        }

        lines.push("\nUse the available tools (ls, read, write, edit, grep, run, glob) to explore and modify files.".to_string());
        lines.push(
            "Always read a file before editing it. Prefer the glob tool over 'find' or 'ls -R'."
                .to_string(),
        );

        // Inject project instruction file content
        if let Some((filename, content)) = &self.project_md {
            lines.push(format!(
                "\n---\n# Project Instructions (from {})\n",
                filename
            ));
            lines.push(content.clone());
        }

        lines.join("\n")
    }
}

fn detect_project_type(root: &Path) -> String {
    if root.join("Cargo.toml").exists() {
        "Rust".to_string()
    } else if root.join("package.json").exists() {
        "Node.js".to_string()
    } else if root.join("setup.py").exists() || root.join("pyproject.toml").exists() {
        "Python".to_string()
    } else if root.join("go.mod").exists() {
        "Go".to_string()
    } else if root.join("pom.xml").exists() || root.join("build.gradle").exists() {
        "Java".to_string()
    } else if root.join("CMakeLists.txt").exists() {
        "C/C++ (CMake)".to_string()
    } else if root.join("Makefile").exists() {
        "C/C++ (Make)".to_string()
    } else {
        "Unknown".to_string()
    }
}

fn detect_project_name(root: &Path, project_type: &str) -> String {
    match project_type {
        "Rust" => {
            if let Ok(content) = fs::read_to_string(root.join("Cargo.toml")) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if let Some(name) = trimmed.strip_prefix("name = \"") {
                        if let Some(end) = name.find('"') {
                            return name[..end].to_string();
                        }
                    }
                    if let Some(name) = trimmed.strip_prefix("name = '") {
                        if let Some(end) = name.find('\'') {
                            return name[..end].to_string();
                        }
                    }
                }
            }
            root.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown".to_string())
        }
        "Node.js" => {
            if let Ok(content) = fs::read_to_string(root.join("package.json")) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(name) = json.get("name").and_then(|n| n.as_str()) {
                        return name.to_string();
                    }
                }
            }
            root.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown".to_string())
        }
        _ => root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
    }
}

fn list_top_level(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            // Skip hidden files/dirs and common ignored directories
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }

            if path.is_dir() {
                // Show dir with trailing slash and list one level of contents
                let mut children: Vec<String> = Vec::new();
                if let Ok(sub_dir) = fs::read_dir(&path) {
                    for sub in sub_dir.flatten() {
                        let sub_name = sub
                            .path()
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if !sub_name.starts_with('.') {
                            children.push(sub_name);
                        }
                    }
                }
                children.sort();
                if children.len() <= 6 {
                    let child_list = children.join(", ");
                    entries.push(format!("{}/  ({})", name, child_list));
                } else {
                    entries.push(format!("{}/  ({} entries)", name, children.len()));
                }
            } else {
                entries.push(name);
            }
        }
    }

    entries.sort();
    entries
}

fn detect_commands(project_type: &str) -> (String, String) {
    match project_type {
        "Rust" => ("cargo build".to_string(), "cargo test".to_string()),
        "Node.js" => ("npm run build".to_string(), "npm test".to_string()),
        "Python" => ("pip install -e .".to_string(), "pytest".to_string()),
        "Go" => ("go build ./...".to_string(), "go test ./...".to_string()),
        _ => (String::new(), String::new()),
    }
}

/// Search for a project instruction file in the current directory and parent
/// directories up to the git root (or filesystem root). Returns the first
/// match found, following the priority order defined in `PROJECT_MD_FILE_NAMES`.
///
/// This mirrors how CLAUDE.md and HERMES.md discover context files:
/// walk up from CWD, check each directory for any of the known filenames.
fn discover_project_md(start_dir: &Path) -> Option<(String, String)> {
    let mut dir = start_dir.to_path_buf();

    loop {
        for &filename in PROJECT_MD_FILE_NAMES {
            let candidate = dir.join(filename);
            if candidate.is_file() {
                if let Ok(content) = fs::read_to_string(&candidate) {
                    let truncated = truncate_content(&content, filename);
                    return Some((filename.to_string(), truncated));
                }
            }
        }

        // Walk up one directory
        if let Some(parent) = dir.parent() {
            // Stop at filesystem root
            if parent == dir {
                break;
            }
            dir = parent.to_path_buf();

            // Stop at git root boundary: if we just checked inside a .git
            // directory's parent, we've reached the repo root.
            // We continue walking up because CLAUDE.md walks up to root,
            // but we stop at the filesystem root.
        } else {
            break;
        }
    }

    None
}

/// Truncate content that exceeds `PROJECT_MD_MAX_CHARS`.
/// Uses a head/tail strategy (70% head, 20% tail, with a 10% truncation marker)
/// to preserve both the beginning (which usually contains the most important
/// instructions) and the end (which may contain verification steps or gotchas).
fn truncate_content(content: &str, filename: &str) -> String {
    if content.len() <= PROJECT_MD_MAX_CHARS {
        return content.to_string();
    }

    let head_end = (PROJECT_MD_MAX_CHARS as f64 * PROJECT_MD_HEAD_RATIO) as usize;
    let tail_size = (PROJECT_MD_MAX_CHARS as f64 * (1.0 - PROJECT_MD_HEAD_RATIO)) as usize;

    let head = &content[..content.floor_char_boundary(head_end)];
    let tail_start = content.len().saturating_sub(tail_size);
    let tail = &content[content.floor_char_boundary(tail_start)..];

    let total = content.len();
    let kept_head = head.len();
    let kept_tail = tail.len();

    format!(
        "{head}\n\n[...truncated {filename}: kept {kept_head}+{kept_tail} of {total} chars. Use the read tool to view the full file.]\n\n{tail}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Test that TINYHARNESS.md is discovered from the current directory.
    #[test]
    fn test_discover_project_md_tinyharness_md() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("TINYHARNESS.md"), "# Project\n\nUse Rust.").unwrap();

        let result = discover_project_md(dir_path);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "TINYHARNESS.md");
        assert!(content.contains("# Project"));
        assert!(content.contains("Use Rust."));
    }

    /// Test priority: TINYHARNESS.md takes precedence over AGENTS.md.
    #[test]
    fn test_discover_project_md_priority() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("TINYHARNESS.md"), "# From TINYHARNESS.md").unwrap();
        fs::write(dir_path.join("AGENTS.md"), "# From AGENTS.md").unwrap();
        fs::write(dir_path.join("CLAUDE.md"), "# From CLAUDE.md").unwrap();

        let result = discover_project_md(dir_path);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "TINYHARNESS.md");
        assert!(content.contains("From TINYHARNESS.md"));
    }

    /// Test fallback: AGENTS.md is found when TINYHARNESS.md doesn't exist.
    #[test]
    fn test_discover_project_md_agents_md_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("AGENTS.md"), "# From AGENTS.md").unwrap();

        let result = discover_project_md(dir_path);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "AGENTS.md");
        assert!(content.contains("From AGENTS.md"));
    }

    /// Test fallback: CLAUDE.md is found when higher-priority files don't exist.
    #[test]
    fn test_discover_project_md_claude_md_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("CLAUDE.md"), "# From CLAUDE.md").unwrap();

        let result = discover_project_md(dir_path);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "CLAUDE.md");
        assert!(content.contains("From CLAUDE.md"));
    }

    /// Test that no file returns None.
    #[test]
    fn test_discover_project_md_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_project_md(dir.path());
        assert!(result.is_none());
    }

    /// Test walking up to parent directories to find the file.
    #[test]
    fn test_discover_project_md_walks_up() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        // Put TINYHARNESS.md in the root, but search from a subdirectory
        fs::write(dir_path.join("TINYHARNESS.md"), "# Found in parent").unwrap();

        let subdir = dir_path.join("src").join("tools");
        fs::create_dir_all(&subdir).unwrap();

        let result = discover_project_md(&subdir);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "TINYHARNESS.md");
        assert!(content.contains("Found in parent"));
    }

    /// Test that .tinyharness.md (hidden variant) is found.
    #[test]
    fn test_discover_project_md_hidden_variant() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join(".tinyharness.md"), "# Hidden variant").unwrap();

        let result = discover_project_md(dir_path);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, ".tinyharness.md");
        assert!(content.contains("Hidden variant"));
    }

    /// Test truncation of oversized content.
    #[test]
    fn test_truncate_content_under_limit() {
        let content = "Hello, world!".to_string();
        let result = truncate_content(&content, "TINYHARNESS.md");
        assert_eq!(result, content); // No truncation needed
    }

    /// Test truncation of content that exceeds the limit.
    #[test]
    fn test_truncate_content_over_limit() {
        // Create content that exceeds the limit
        let content = "A".repeat(PROJECT_MD_MAX_CHARS + 5000);
        let result = truncate_content(&content, "TINYHARNESS.md");

        // Should contain the truncation marker
        assert!(result.contains("[...truncated TINYHARNESS.md"));
        assert!(result.contains("Use the read tool to view the full file"));

        // Total result should be smaller than the original
        assert!(result.len() < content.len());

        // Should start with the head (A's) and end with the tail (A's)
        assert!(result.starts_with('A'));
        assert!(result.ends_with('A'));
    }

    /// Test that format() includes project_md content when present.
    #[test]
    fn test_format_includes_project_md() {
        let ctx = WorkspaceContext {
            root: PathBuf::from("/tmp/test"),
            project_type: "Rust".to_string(),
            project_name: "test-project".to_string(),
            structure: vec!["src/  (main.rs)".to_string()],
            is_git_repo: false,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            project_md: Some((
                "TINYHARNESS.md".to_string(),
                "# My Rules\nAlways use Rust.".to_string(),
            )),
        };

        let formatted = ctx.format();
        assert!(formatted.contains("# Project Instructions (from TINYHARNESS.md)"));
        assert!(formatted.contains("# My Rules"));
        assert!(formatted.contains("Always use Rust."));
    }

    /// Test that format() works when no project_md is found.
    #[test]
    fn test_format_without_project_md() {
        let ctx = WorkspaceContext {
            root: PathBuf::from("/tmp/test"),
            project_type: "Rust".to_string(),
            project_name: "test-project".to_string(),
            structure: vec!["src/  (main.rs)".to_string()],
            is_git_repo: false,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            project_md: None,
        };

        let formatted = ctx.format();
        assert!(!formatted.contains("Project Instructions"));
    }

    /// Test priority between .tinyharness.md and AGENTS.md.
    #[test]
    fn test_discover_project_md_hidden_over_agents() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join(".tinyharness.md"), "# Hidden").unwrap();
        fs::write(dir_path.join("AGENTS.md"), "# Agents").unwrap();

        let result = discover_project_md(dir_path);
        assert!(result.is_some());
        let (filename, _) = result.unwrap();
        assert_eq!(filename, ".tinyharness.md");
    }
}
