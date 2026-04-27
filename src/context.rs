use std::{
    fs,
    path::{Path, PathBuf},
};

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

        WorkspaceContext {
            root,
            project_type,
            project_name,
            structure,
            is_git_repo,
            build_command,
            test_command,
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
        lines.push("Always read a file before editing it. Prefer the glob tool over 'find' or 'ls -R'.".to_string());

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
                    entries.push(format!(
                        "{}/  ({} entries)",
                        name,
                        children.len()
                    ));
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
