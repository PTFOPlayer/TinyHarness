use std::path::PathBuf;

use crate::context::{WorkspaceContext, PROJECT_MD_FILE_NAMES};
use crate::provider::{Message, Role};
use crate::style::*;

/// Result of the `/init` command.
pub enum InitResult {
    /// The file was created from scratch.
    Created { path: PathBuf },
    /// The existing file was updated.
    Updated { path: PathBuf },
}

/// Generate or update a project instruction file (TINYHARNESS.md, etc.)
/// using the LLM provider to analyze the codebase.
///
/// This mirrors how Claude Code's `/init` works: the AI explores the project
/// and generates/updates the instruction file with build commands, test
/// instructions, project conventions, and architecture notes.
pub async fn execute_init(
    provider: &mut dyn crate::provider::Provider,
    workspace_ctx: &WorkspaceContext,
    _messages: &mut Vec<Message>,
) -> Result<InitResult, String> {
    let root = &workspace_ctx.root;

    // Check if a project instruction file already exists
    let existing = find_existing_project_md(root);

    let (action_label, existing_content) = match &existing {
        Some((filename, path)) => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read {}: {}", filename, e))?;
            println!(
                "{}  Found existing {}{}{} ({} bytes). Updating...{}",
                BOLD, BLUE, filename, RESET, content.len(), RESET
            );
            ("Updating", Some(content))
        }
        None => {
            println!(
                "{}  Generating project instruction file...{}",
                BOLD, RESET
            );
            ("Creating", None)
        }
    };

    // Build the prompt asking the LLM to generate/update the file
    let prompt = build_init_prompt(workspace_ctx, existing_content.as_deref());

    // Build messages for the LLM call
    let init_messages = vec![
        Message {
            role: Role::System,
            content: "You are a project analyst AI. Your task is to generate or update a project instruction file \
                       (similar to CLAUDE.md or AGENTS.md) that gives an AI assistant persistent context about \
                       this project. Be specific, concise, and factual. Focus on things an AI cannot infer from \
                       the code alone: build commands, conventions, gotchas, architecture decisions. \
                       Output ONLY the raw markdown content — no code fences, no explanations before or after.".to_string(),
            tool_calls: vec![],
        },
        Message {
            role: Role::User,
            content: prompt,
            tool_calls: vec![],
        },
    ];

    println!(
        "{}  {} — analyzing project...{}",
        CYAN, action_label, RESET
    );

    // Call the provider to generate the content
    let (send, mut recv) = tokio::sync::mpsc::channel::<crate::provider::ChatMessageResponse>(1024);
    let tools = vec![]; // No tools needed for generation

    provider
        .chat(init_messages, "Generate project instruction file".to_string(), send, tools)
        .await;

    // Collect the response
    let mut generated_content = String::new();
    let mut done = false;
    while let Some(msg) = recv.recv().await {
        if !msg.message.content.is_empty() {
            generated_content.push_str(&msg.message.content);
        }
        if msg.done {
            done = true;
            break;
        }
    }

    if !done || generated_content.is_empty() {
        return Err("Failed to generate project instruction file. The LLM did not respond.".to_string());
    }

    // Strip markdown code fences if the LLM wrapped them
    let generated_content = strip_code_fences(&generated_content);

    // Determine the target filename and path
    let (filename, path) = match &existing {
        Some((fn_name, p)) => (fn_name.clone(), p.clone()),
        None => ("TINYHARNESS.md".to_string(), root.join("TINYHARNESS.md")),
    };

    // Write the file
    std::fs::write(&path, &generated_content)
        .map_err(|e| format!("Failed to write {}: {}", filename, e))?;

    // Print success
    let line_count = generated_content.lines().count();
    match action_label {
        "Creating" => {
            println!(
                "\n{}  ✦ Created {}{}{} ({} lines){}",
                GREEN, BLUE, path.display(), GREEN, line_count, RESET
            );
        }
        "Updating" => {
            println!(
                "\n{}  ✦ Updated {}{}{} ({} lines){}",
                GREEN, BLUE, path.display(), GREEN, line_count, RESET
            );
        }
        _ => {}
    }

    // Print a preview of the first few lines
    println!();
    let preview_lines: Vec<&str> = generated_content.lines().take(6).collect();
    for line in &preview_lines {
        println!("{}  {}{}", GRAY, line, RESET);
    }
    if generated_content.lines().count() > 6 {
        println!(
            "{}  ... ({} more lines){}",
            GRAY,
            generated_content.lines().count() - 6,
            RESET
        );
    }
    println!();

    Ok(match existing {
        Some(_) => InitResult::Updated { path },
        None => InitResult::Created { path },
    })
}

/// Find an existing project instruction file in the workspace root.
fn find_existing_project_md(root: &std::path::Path) -> Option<(String, PathBuf)> {
    for &filename in PROJECT_MD_FILE_NAMES {
        let candidate = root.join(filename);
        if candidate.is_file() {
            return Some((filename.to_string(), candidate));
        }
    }
    None
}

/// Build the prompt for generating/updating the project instruction file.
fn build_init_prompt(ctx: &WorkspaceContext, existing_content: Option<&str>) -> String {
    let mut prompt = String::new();

    prompt.push_str("Analyze this project and generate a project instruction file (TINYHARNESS.md format).\n\n");

    // Project metadata
    prompt.push_str(&format!("Project name: {}\n", ctx.project_name));
    prompt.push_str(&format!("Project type: {}\n", ctx.project_type));
    prompt.push_str(&format!("Workspace root: {}\n", ctx.root.display()));
    if ctx.is_git_repo {
        prompt.push_str("This is a git repository.\n");
    }
    if !ctx.build_command.is_empty() {
        prompt.push_str(&format!("Build command: {}\n", ctx.build_command));
    }
    if !ctx.test_command.is_empty() {
        prompt.push_str(&format!("Test command: {}\n", ctx.test_command));
    }

    // Directory structure
    prompt.push_str("\nTop-level directory structure:\n");
    for entry in &ctx.structure {
        prompt.push_str(&format!("  {}\n", entry));
    }

    // If there's existing content, ask the LLM to update it
    if let Some(content) = existing_content {
        prompt.push_str("\n---\n");
        prompt.push_str("The project already has an instruction file. Here is the current content:\n\n");
        prompt.push_str(content);
        prompt.push_str("\n---\n\n");
        prompt.push_str("Please UPDATE this file. Keep what's still accurate, remove what's outdated, \
                          and add anything missing. Focus on:\n");
        prompt.push_str("1. Build/test/lint commands that are specific and correct\n");
        prompt.push_str("2. Code conventions that differ from defaults\n");
        prompt.push_str("3. Architecture overview (key directories, module relationships)\n");
        prompt.push_str("4. Known gotchas and important rules\n");
        prompt.push_str("5. Verification steps to run after making changes\n\n");
        prompt.push_str("Keep it concise (under 200 lines). Remove anything that's obvious from the code itself.\n");
    } else {
        prompt.push_str("\nPlease generate a new project instruction file. Include:\n");
        prompt.push_str("1. Project overview (one-line description + tech stack)\n");
        prompt.push_str("2. Commands section (build, test, lint, run — specific commands, not vague)\n");
        prompt.push_str("3. Code Conventions section (rules that differ from language defaults)\n");
        prompt.push_str("4. Architecture section (key directories, module relationships, how things connect)\n");
        prompt.push_str("5. Testing section (framework, patterns, how to run specific tests)\n");
        prompt.push_str("6. Known Gotchas section (things that trip up newcomers)\n");
        prompt.push_str("7. Important Rules section (hard constraints the AI must follow)\n\n");
        prompt.push_str("Keep it concise (under 200 lines). Only include things an AI cannot infer from the code itself. \
                          Use markdown headers and bullet points for structure.\n");
    }

    prompt
}

/// Strip markdown code fences if the LLM wrapped its output in them.
/// Handles ```markdown, ```md, and bare ``` fences.
fn strip_code_fences(content: &str) -> String {
    let trimmed = content.trim();

    // Check if the content starts with ``` and ends with ```
    if !trimmed.starts_with("```") || !trimmed.ends_with("```") {
        return content.to_string();
    }

    // Find the end of the first line (the opening fence, possibly with a language tag)
    let first_newline = trimmed.find('\n');
    let content_start = match first_newline {
        Some(pos) => pos + 1, // Skip past the opening ```\n or ```lang\n line
        None => return content.to_string(), // No newline — single-line fence, return as-is
    };

    // Find the closing ```. It should be at the end after trimming.
    // Walk backwards from the end to find the start of the closing fence line.
    let inner = trimmed[content_start..].trim_end();
    // Remove trailing ``` if present
    if inner.ends_with("```") {
        let without_closing = inner[..inner.len() - 3].trim_end();
        return without_closing.trim().to_string();
    }

    // No proper closing fence found inside — return as-is
    content.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fences_markdown() {
        let input = "```markdown\n# My Project\n\nSome content\n```";
        let result = strip_code_fences(input);
        assert_eq!(result, "# My Project\n\nSome content");
    }

    #[test]
    fn test_strip_code_fences_md() {
        let input = "```md\n# My Project\n\nSome content\n```";
        let result = strip_code_fences(input);
        assert_eq!(result, "# My Project\n\nSome content");
    }

    #[test]
    fn test_strip_code_fences_bare() {
        let input = "```\n# My Project\n\nSome content\n```";
        let result = strip_code_fences(input);
        assert_eq!(result, "# My Project\n\nSome content");
    }

    #[test]
    fn test_strip_code_fences_none() {
        let input = "# My Project\n\nSome content";
        let result = strip_code_fences(input);
        assert_eq!(result, "# My Project\n\nSome content");
    }

    #[test]
    fn test_strip_code_fences_with_leading_whitespace() {
        let input = "  ```markdown\n# My Project\n```  ";
        let result = strip_code_fences(input);
        assert_eq!(result, "# My Project");
    }

    #[test]
    fn test_find_existing_project_md_tinyharness() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TINYHARNESS.md"), "# Test").unwrap();
        let result = find_existing_project_md(dir.path());
        assert!(result.is_some());
        let (filename, _) = result.unwrap();
        assert_eq!(filename, "TINYHARNESS.md");
    }

    #[test]
    fn test_find_existing_project_md_agents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Test").unwrap();
        let result = find_existing_project_md(dir.path());
        assert!(result.is_some());
        let (filename, _) = result.unwrap();
        assert_eq!(filename, "AGENTS.md");
    }

    #[test]
    fn test_find_existing_project_md_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_existing_project_md(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_find_existing_project_md_priority() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TINYHARNESS.md"), "# TH").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# AG").unwrap();
        let result = find_existing_project_md(dir.path());
        assert!(result.is_some());
        let (filename, _) = result.unwrap();
        assert_eq!(filename, "TINYHARNESS.md"); // TINYHARNESS.md has priority
    }

    #[test]
    fn test_strip_code_fences_multiline_content() {
        let input = "```markdown\n# Title\n\nParagraph\n\n- Item 1\n- Item 2\n```";
        let result = strip_code_fences(input);
        assert_eq!(result, "# Title\n\nParagraph\n\n- Item 1\n- Item 2");
    }

    #[test]
    fn test_strip_code_fences_empty_content() {
        let input = "```\n```";
        let result = strip_code_fences(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_code_fences_no_closing_fence() {
        let input = "```markdown\n# My Project";
        let result = strip_code_fences(input);
        assert_eq!(result, "```markdown\n# My Project"); // No closing fence, return as-is
    }

    #[test]
    fn test_build_init_prompt_new() {
        use crate::context::WorkspaceContext;
        use std::path::PathBuf;

        let ctx = WorkspaceContext {
            root: PathBuf::from("/tmp/test"),
            project_type: "Rust".to_string(),
            project_name: "my-project".to_string(),
            structure: vec!["src/  (main.rs)".to_string()],
            is_git_repo: true,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            project_md: None,
        };

        let prompt = build_init_prompt(&ctx, None);
        assert!(prompt.contains("my-project"));
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("cargo build"));
        assert!(prompt.contains("cargo test"));
        assert!(prompt.contains("generate a new project instruction file"));
    }

    #[test]
    fn test_build_init_prompt_update() {
        use crate::context::WorkspaceContext;
        use std::path::PathBuf;

        let ctx = WorkspaceContext {
            root: PathBuf::from("/tmp/test"),
            project_type: "Rust".to_string(),
            project_name: "my-project".to_string(),
            structure: vec!["src/  (main.rs)".to_string()],
            is_git_repo: true,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            project_md: None,
        };

        let existing = "# Old Rules\nUse cargo.";
        let prompt = build_init_prompt(&ctx, Some(existing));
        assert!(prompt.contains("UPDATE this file"));
        assert!(prompt.contains("Old Rules"));
    }
}