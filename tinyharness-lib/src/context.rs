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
pub const DEFAULT_PROJECT_MD_FILE_NAMES: &[&str] = &[
    "TINYHARNESS.md",
    ".tinyharness.md",
    "AGENTS.md",
    "CLAUDE.md",
];

/// Compatibility alias — kept for existing internal references.
pub const PROJECT_MD_FILE_NAMES: &[&str] = DEFAULT_PROJECT_MD_FILE_NAMES;

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
    /// Additional project MD files loaded from `.tinyharness/config.json`'s
    /// `project_md_files` field (e.g. RULES.md, .cursorrules).
    /// Each entry is (filename, content). Empty if none configured.
    pub additional_project_mds: Vec<(String, String)>,
}

impl WorkspaceContext {
    /// Collect workspace context from the current working directory.
    pub fn collect() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_type = detect_project_type(&root);
        let project_name = detect_project_name(&root, &project_type);
        let structure = list_top_level(&root);
        let is_git_repo = root.join(".git").is_dir();
        let (build_command, test_command) = detect_commands(&project_type, &root);

        // Resolve the project MD discovery list (env var > settings > defaults)
        let settings = crate::config::load_settings();
        let md_files = crate::config::resolve_project_md_files(Some(&settings));
        let project_md = discover_project_md(&root, &md_files);

        // Load additional MD files from per-project .tinyharness/config.json
        let additional_project_mds = if let Some(Ok(proj)) =
            crate::config::discover_project_settings(&root)
        {
            load_additional_md_files(&root, proj.project_md_files.as_deref())
        } else {
            Vec::new()
        };

        WorkspaceContext {
            root,
            project_type: project_type.to_string(),
            project_name,
            structure,
            is_git_repo,
            build_command: build_command.to_string(),
            test_command: test_command.to_string(),
            project_md,
            additional_project_mds,
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

        if let Some((filename, content)) = &self.project_md {
            lines.push(format!("\n---\n# Project Instructions (from {filename})\n"));
            lines.push(content.clone());
        }

        // Append additional project MD files
        for (filename, content) in &self.additional_project_mds {
            lines.push(format!("\n---\n# Additional Instructions (from {filename})\n"));
            lines.push(content.clone());
        }

        lines.join("\n")
    }
}

/// Load additional project instruction files specified in per-project config.
fn load_additional_md_files(root: &Path, files: Option<&[String]>) -> Vec<(String, String)> {
    let Some(files) = files else { return Vec::new() };
    files
        .iter()
        .filter_map(|name| {
            let path = root.join(name);
            if path.is_file()
                && let Ok(content) = fs::read_to_string(&path)
            {
                let truncated = truncate_content(&content, name);
                Some((name.clone(), truncated))
            } else {
                None
            }
        })
        .collect()
}

/// Detection entry: (language label, marker file(s), build command, test command).
/// Ordered by priority — first match wins per detection pass.
struct LanguageSignature {
    label: &'static str,
    markers: &'static [&'static str],
    build_cmd: &'static str,
    test_cmd: &'static str,
}

const LANGUAGE_SIGNATURES: &[LanguageSignature] = &[
    LanguageSignature {
        label: "Rust",
        markers: &["Cargo.toml"],
        build_cmd: "cargo build",
        test_cmd: "cargo test",
    },
    LanguageSignature {
        label: "Zig",
        markers: &["build.zig", "build.zig.zon"],
        build_cmd: "zig build",
        test_cmd: "zig build test",
    },
    LanguageSignature {
        label: "Deno",
        markers: &["deno.json", "deno.jsonc"],
        build_cmd: "deno task build",
        test_cmd: "deno test",
    },
    LanguageSignature {
        label: "Bun",
        markers: &["bun.lockb", "bun.lock"],
        build_cmd: "bun run build",
        test_cmd: "bun test",
    },
    LanguageSignature {
        label: "Swift",
        markers: &["Package.swift"],
        build_cmd: "swift build",
        test_cmd: "swift test",
    },
    LanguageSignature {
        label: "Ruby",
        markers: &["Gemfile"],
        build_cmd: "bundle install",
        test_cmd: "bundle exec rspec",
    },
    LanguageSignature {
        label: "Elixir",
        markers: &["mix.exs"],
        build_cmd: "mix compile",
        test_cmd: "mix test",
    },
    LanguageSignature {
        label: "Haskell",
        markers: &["stack.yaml"],
        build_cmd: "stack build",
        test_cmd: "stack test",
    },
    LanguageSignature {
        label: "Kotlin",
        markers: &["build.gradle.kts", "settings.gradle.kts"],
        build_cmd: "gradle build",
        test_cmd: "gradle test",
    },
    LanguageSignature {
        label: "Dart/Flutter",
        markers: &["pubspec.yaml"],
        build_cmd: "dart run build",
        test_cmd: "dart test",
    },
    LanguageSignature {
        label: "Nix",
        markers: &["flake.nix", "default.nix"],
        build_cmd: "nix build",
        test_cmd: "nix flake check",
    },
    LanguageSignature {
        label: "Node.js",
        markers: &["package.json"],
        build_cmd: "npm run build",
        test_cmd: "npm test",
    },
    LanguageSignature {
        label: "Python",
        markers: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt"],
        build_cmd: "pip install -e .",
        test_cmd: "pytest",
    },
    LanguageSignature {
        label: "Go",
        markers: &["go.mod"],
        build_cmd: "go build ./...",
        test_cmd: "go test ./...",
    },
    LanguageSignature {
        label: "Java (Gradle)",
        markers: &["build.gradle"],
        build_cmd: "gradle build",
        test_cmd: "gradle test",
    },
    LanguageSignature {
        label: "Java (Maven)",
        markers: &["pom.xml"],
        build_cmd: "mvn compile",
        test_cmd: "mvn test",
    },
    LanguageSignature {
        label: "C/C++ (CMake)",
        markers: &["CMakeLists.txt"],
        build_cmd: "cmake --build build",
        test_cmd: "ctest",
    },
];

/// Glob patterns for additional marker files that are checked after the
/// ordered signatures. These detect languages whose markers overlap with
/// other signatures (e.g. `.csproj` for .NET).
const SECONDARY_MARKERS: &[(&str, &str)] = &[
    (".NET", "*.csproj"),
    (".NET", "*.sln"),
    ("Haskell (Cabal)", "*.cabal"),
];

fn detect_project_type(root: &Path) -> String {
    let mut found: Vec<&'static str> = Vec::new();

    for sig in LANGUAGE_SIGNATURES {
        for marker in sig.markers {
            if root.join(marker).exists() {
                if !found.contains(&sig.label) {
                    found.push(sig.label);
                }
                break; // one marker match is enough
            }
        }
    }

    // Secondary check: glob for patterns like *.csproj
    for (label, pattern) in SECONDARY_MARKERS {
        if found.contains(label) {
            continue;
        }
        if let Ok(entries) = glob::glob(&root.join(pattern).to_string_lossy()) {
            if entries.flatten().next().is_some() {
                found.push(label);
            }
        }
    }

    if found.is_empty() {
        // Fallback: check for Makefile / Justfile as task-runner hints
        if root.join("Makefile").exists() {
            "Unknown (has Makefile)".to_string()
        } else if root.join("Justfile").exists() {
            "Unknown (has Justfile)".to_string()
        } else {
            "Unknown".to_string()
        }
    } else if found.len() == 1 {
        found[0].to_string()
    } else {
        // Monorepo: join all detected types
        found.join(" + ")
    }
}

/// Extract a quoted field value (supports both double and single quotes).
fn extract_quoted_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    for (prefix, quote) in [
        (format!("{} = \"", key), '"'),
        (format!("{} = '", key), '\''),
    ] {
        if let Some(name) = trimmed
            .strip_prefix(&prefix)
            .and_then(|n| n.find(quote).map(|end| &n[..end]))
        {
            return Some(name);
        }
    }
    None
}

fn detect_project_name(root: &Path, project_type: &str) -> String {
    let fallback = || {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string())
    };

    // Handle "Rust + Node.js" style monorepo labels
    let primary = project_type.split_once(" + ").map(|(a, _)| a).unwrap_or(project_type);

    match primary {
        "Rust" => {
            if let Ok(content) = fs::read_to_string(root.join("Cargo.toml")) {
                for line in content.lines() {
                    if let Some(name) = extract_quoted_field(line, "name") {
                        return name.to_string();
                    }
                }
            }
            fallback()
        }
        "Node.js" | "Bun" | "Deno" => {
            if let Ok(content) = fs::read_to_string(root.join("package.json"))
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(name) = json.get("name").and_then(|n| n.as_str())
            {
                return name.to_string();
            }
            fallback()
        }
        "Python" => {
            if let Ok(content) = fs::read_to_string(root.join("pyproject.toml")) {
                for line in content.lines() {
                    if let Some(name) = extract_quoted_field(line, "name") {
                        return name.to_string();
                    }
                }
            }
            fallback()
        }
        "Go" => {
            // Read module name from go.mod
            if let Ok(content) = fs::read_to_string(root.join("go.mod")) {
                for line in content.lines() {
                    if let Some(rest) = line.strip_prefix("module ") {
                        let name = rest.trim();
                        if !name.is_empty() {
                            return name.to_string();
                        }
                    }
                }
            }
            fallback()
        }
        _ => fallback(),
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

fn detect_commands(project_type: &str, root: &Path) -> (&'static str, &'static str) {
    // Handle monorepo labels: use the first detected type's commands
    let primary = project_type.split_once(" + ").map(|(a, _)| a).unwrap_or(project_type);

    for sig in LANGUAGE_SIGNATURES {
        if sig.label == primary {
            return (sig.build_cmd, sig.test_cmd);
        }
    }

    // Fallback: check for Makefile / Justfile as task-runner hints
    if root.join("Makefile").exists() {
        ("make", "make test")
    } else if root.join("Justfile").exists() {
        ("just build", "just test")
    } else {
        ("", "")
    }
}

/// Search for a project instruction file in the current directory and parent
/// directories up to the filesystem root. Returns the first
/// match found, following the priority order defined in `file_names`.
///
/// This mirrors how CLAUDE.md and HERMES.md discover context files:
/// walk up from CWD, check each directory for any of the known filenames.
fn discover_project_md(start_dir: &Path, file_names: &[String]) -> Option<(String, String)> {
    if file_names.is_empty() {
        return None;
    }

    let mut dir = start_dir.to_path_buf();

    loop {
        for filename in file_names {
            let candidate = dir.join(filename);
            if candidate.is_file()
                && let Ok(content) = fs::read_to_string(&candidate)
            {
                let truncated = truncate_content(&content, filename);
                return Some((filename.to_string(), truncated));
            }
        }

        // Walk up one directory
        if let Some(parent) = dir.parent() {
            if parent == dir {
                break;
            }
            dir = parent.to_path_buf();
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

    /// Helper: the default discovery list as owned Strings for tests.
    fn default_md_names() -> Vec<String> {
        DEFAULT_PROJECT_MD_FILE_NAMES.iter().map(|s| s.to_string()).collect()
    }

    /// Helper: custom discovery list for override tests.
    fn custom_md_names(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// Test that TINYHARNESS.md is discovered from the current directory.
    #[test]
    fn test_discover_project_md_tinyharness_md() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("TINYHARNESS.md"), "# Project\n\nUse Rust.").unwrap();

        let result = discover_project_md(dir_path, &default_md_names());
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

        let result = discover_project_md(dir_path, &default_md_names());
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

        let result = discover_project_md(dir_path, &default_md_names());
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

        let result = discover_project_md(dir_path, &default_md_names());
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "CLAUDE.md");
        assert!(content.contains("From CLAUDE.md"));
    }

    /// Test that no file returns None.
    #[test]
    fn test_discover_project_md_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_project_md(dir.path(), &default_md_names());
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

        let result = discover_project_md(&subdir, &default_md_names());
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

        let result = discover_project_md(dir_path, &default_md_names());
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, ".tinyharness.md");
        assert!(content.contains("Hidden variant"));
    }

    /// Test that custom file names work (env var / settings override).
    #[test]
    fn test_discover_project_md_custom_names() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("TEAM_RULES.md"), "# Team Rules").unwrap();
        fs::write(dir_path.join("CLAUDE.md"), "# Claude Rules").unwrap();

        // Custom order: TEAM_RULES.md first
        let names = custom_md_names(&["TEAM_RULES.md", "CLAUDE.md"]);
        let result = discover_project_md(dir_path, &names);
        assert!(result.is_some());
        let (filename, content) = result.unwrap();
        assert_eq!(filename, "TEAM_RULES.md");
        assert!(content.contains("# Team Rules"));
    }

    /// Test that when custom list is empty, nothing is found.
    #[test]
    fn test_discover_project_md_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join("TINYHARNESS.md"), "# Project").unwrap();

        let result = discover_project_md(dir_path, &[]);
        assert!(result.is_none());
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
            additional_project_mds: Vec::new(),
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
            additional_project_mds: Vec::new(),
        };

        let formatted = ctx.format();
        assert!(!formatted.contains("Project Instructions"));
    }

    /// Test that format() includes additional project MD files.
    #[test]
    fn test_format_includes_additional_mds() {
        let ctx = WorkspaceContext {
            root: PathBuf::from("/tmp/test"),
            project_type: "Rust".to_string(),
            project_name: "test-project".to_string(),
            structure: vec!["src/  (main.rs)".to_string()],
            is_git_repo: false,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            project_md: None,
            additional_project_mds: vec![
                ("RULES.md".to_string(), "# Custom Rules".to_string()),
            ],
        };

        let formatted = ctx.format();
        assert!(formatted.contains("# Additional Instructions (from RULES.md)"));
        assert!(formatted.contains("# Custom Rules"));
    }

    /// Test priority between .tinyharness.md and AGENTS.md.
    #[test]
    fn test_discover_project_md_hidden_over_agents() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        fs::write(dir_path.join(".tinyharness.md"), "# Hidden").unwrap();
        fs::write(dir_path.join("AGENTS.md"), "# Agents").unwrap();

        let result = discover_project_md(dir_path, &default_md_names());
        assert!(result.is_some());
        let (filename, _) = result.unwrap();
        assert_eq!(filename, ".tinyharness.md");
    }
}
