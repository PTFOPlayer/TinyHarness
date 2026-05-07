use std::collections::HashMap;
use std::process::Command;

use crate::define_tool;
use crate::tools::tool::BoxFuture;

/// Check if a directory is inside a git repository.
fn is_git_repo(path: &str) -> Result<(), String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(path)
        .output()
        .map_err(|e| format!("Error: git command failed: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Error: '{}' is not a git repository", path))
    }
}

/// Execute a git command and return the output as a string.
fn run_git_command(path: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|e| format!("Error: git command failed: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Error: git command failed: {}", stderr.trim()))
    }
}

pub fn git_status_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".to_string());

        // Check if it's a git repository
        if let Err(e) = is_git_repo(&path) {
            return e;
        }

        // Get current branch
        let branch = match run_git_command(&path, &["branch", "--show-current"]) {
            Ok(b) => b.trim().to_string(),
            Err(_) => "HEAD (detached)".to_string(),
        };

        // Get status in porcelain format (machine-readable)
        let status = match run_git_command(&path, &["status", "--porcelain"]) {
            Ok(s) => s,
            Err(e) => return e,
        };

        // Build formatted output
        let mut output = format!("Branch: {}\n\n", branch);

        if status.is_empty() {
            output.push_str("Working tree clean.\n");
        } else {
            output.push_str("Changes:\n");
            for line in status.lines() {
                if line.is_empty() {
                    continue;
                }
                // Parse porcelain format: XY filename
                // X = staged, Y = unstaged
                let status_chars = &line[..2];
                let filename = line[3..].to_string();

                let status_desc = match status_chars {
                    "M " => "modified (staged)  ",
                    " M" => "modified (unstaged)",
                    "A " => "added (staged)     ",
                    " A" => "added (unstaged)   ",
                    "D " => "deleted (staged)   ",
                    " D" => "deleted (unstaged) ",
                    "R " => "renamed (staged)   ",
                    " R" => "renamed (unstaged) ",
                    "C " => "copied (staged)    ",
                    " C" => "copied (unstaged)  ",
                    "??" => "untracked          ",
                    "!!" => "ignored            ",
                    "UU" => "conflicted         ",
                    _ => "changed              ",
                };

                output.push_str(&format!("  {} {}\n", status_desc, filename));
            }
        }

        output.trim_end().to_string()
    })
}

pub fn git_diff_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".to_string());
        let cached = args.get("cached").map(|s| s == "true").unwrap_or(false);
        let target = args
            .get("target")
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty());

        // Check if it's a git repository
        if let Err(e) = is_git_repo(&path) {
            return e;
        }

        // Build git diff command
        let mut git_args = vec!["diff"];

        if cached {
            git_args.push("--cached");
        }

        if let Some(t) = target {
            git_args.push(t);
        }

        // Execute git diff
        let diff_output = match run_git_command(&path, &git_args) {
            Ok(d) => d,
            Err(e) => return e,
        };

        if diff_output.is_empty() {
            return "No differences found.".to_string();
        }

        // Limit output size to avoid overwhelming responses
        const MAX_LINES: usize = 300;
        let lines: Vec<&str> = diff_output.lines().collect();

        if lines.len() > MAX_LINES {
            let truncated: String = lines[..MAX_LINES].join("\n");
            format!(
                "{}\n\n... (truncated: {} lines total, showing first {})",
                truncated,
                lines.len(),
                MAX_LINES
            )
        } else {
            diff_output.trim_end().to_string()
        }
    })
}

define_tool!(
    git_status_tool_entry, "git_status",
    "Show git repository status including current branch, staged changes, unstaged changes, and untracked files. Returns a formatted summary of the working tree state.",
    required: [],
    optional: [
        ("path", "The directory to run git status in (defaults to current directory)", "."),
    ],
    handler: git_status_tool
);

define_tool!(
    git_diff_tool_entry, "git_diff",
    "Show git differences between working directory and HEAD, staged changes, or a specific target commit/branch. Returns the full diff output.",
    required: [],
    optional: [
        ("path", "The directory to run git diff in (defaults to current directory)", "."),
        ("cached", "If true, show staged changes instead of unstaged (true/false)", "false"),
        ("target", "Optional target to diff against (e.g., HEAD~1, commit hash, branch name). If empty, diffs against HEAD", ""),
    ],
    handler: git_diff_tool
);

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    /// Initialize a git repository in a temp directory with basic config.
    fn init_git_repo(dir: &TempDir) {
        Command::new("git")
            .arg("init")
            .current_dir(dir.path())
            .output()
            .expect("Failed to init git repo");
        Command::new("git")
            .arg("config")
            .arg("user.email")
            .arg("test@example.com")
            .current_dir(dir.path())
            .output()
            .expect("Failed to set git email");
        Command::new("git")
            .arg("config")
            .arg("user.name")
            .arg("Test User")
            .current_dir(dir.path())
            .output()
            .expect("Failed to set git name");
    }

    /// Create a file and commit it to the repo.
    fn create_and_commit(dir: &TempDir, filename: &str, content: &str) {
        let path = dir.path().join(filename);
        fs::write(&path, content).expect("Failed to write file");
        Command::new("git")
            .arg("add")
            .arg(filename)
            .current_dir(dir.path())
            .output()
            .expect("Failed to git add");
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(format!("Add {}", filename))
            .current_dir(dir.path())
            .output()
            .expect("Failed to git commit");
    }

    #[tokio::test]
    async fn test_git_status_not_a_repo() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        // Don't initialize git

        let args = HashMap::from([(
            "path".to_string(),
            temp_dir.path().to_string_lossy().to_string(),
        )]);

        let result = git_status_tool(args).await;
        assert!(result.contains("not a git repository"));
    }

    #[tokio::test]
    async fn test_git_status_clean() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        init_git_repo(&temp_dir);
        create_and_commit(&temp_dir, "test.txt", "initial content");

        let args = HashMap::from([(
            "path".to_string(),
            temp_dir.path().to_string_lossy().to_string(),
        )]);

        let result = git_status_tool(args).await;
        assert!(result.contains("Branch: master") || result.contains("Branch: main"));
        assert!(result.contains("Working tree clean"));
    }

    #[tokio::test]
    async fn test_git_status_with_changes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        init_git_repo(&temp_dir);
        create_and_commit(&temp_dir, "test.txt", "initial content");

        // Modify the file (unstaged)
        fs::write(temp_dir.path().join("test.txt"), "modified content")
            .expect("Failed to modify file");

        // Create an untracked file
        fs::write(temp_dir.path().join("new.txt"), "new file").expect("Failed to create new file");

        let args = HashMap::from([(
            "path".to_string(),
            temp_dir.path().to_string_lossy().to_string(),
        )]);

        let result = git_status_tool(args).await;
        assert!(result.contains("modified (unstaged)"));
        assert!(result.contains("untracked"));
        assert!(result.contains("test.txt"));
        assert!(result.contains("new.txt"));
    }

    #[tokio::test]
    async fn test_git_diff_not_a_repo() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        // Don't initialize git

        let args = HashMap::from([(
            "path".to_string(),
            temp_dir.path().to_string_lossy().to_string(),
        )]);

        let result = git_diff_tool(args).await;
        assert!(result.contains("not a git repository"));
    }

    #[tokio::test]
    async fn test_git_diff_no_changes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        init_git_repo(&temp_dir);
        create_and_commit(&temp_dir, "test.txt", "initial content");

        let args = HashMap::from([(
            "path".to_string(),
            temp_dir.path().to_string_lossy().to_string(),
        )]);

        let result = git_diff_tool(args).await;
        assert!(result.contains("No differences found"));
    }

    #[tokio::test]
    async fn test_git_diff_with_changes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        init_git_repo(&temp_dir);
        create_and_commit(&temp_dir, "test.txt", "initial content");

        // Modify the file
        fs::write(temp_dir.path().join("test.txt"), "modified content")
            .expect("Failed to modify file");

        let args = HashMap::from([(
            "path".to_string(),
            temp_dir.path().to_string_lossy().to_string(),
        )]);

        let result = git_diff_tool(args).await;
        assert!(result.contains("diff --git"));
        assert!(result.contains("test.txt"));
        assert!(result.contains("-initial content"));
        assert!(result.contains("+modified content"));
    }

    #[tokio::test]
    async fn test_git_diff_cached() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        init_git_repo(&temp_dir);
        create_and_commit(&temp_dir, "test.txt", "initial content");

        // Modify and stage the file
        fs::write(temp_dir.path().join("test.txt"), "staged content")
            .expect("Failed to modify file");
        Command::new("git")
            .arg("add")
            .arg("test.txt")
            .current_dir(temp_dir.path())
            .output()
            .expect("Failed to git add");

        let args = HashMap::from([
            (
                "path".to_string(),
                temp_dir.path().to_string_lossy().to_string(),
            ),
            ("cached".to_string(), "true".to_string()),
        ]);

        let result = git_diff_tool(args).await;
        assert!(result.contains("diff --git"));
        assert!(result.contains("test.txt"));
        assert!(result.contains("-initial content"));
        assert!(result.contains("+staged content"));
    }

    #[tokio::test]
    async fn test_git_diff_with_target() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        init_git_repo(&temp_dir);
        create_and_commit(&temp_dir, "test.txt", "first commit");
        create_and_commit(&temp_dir, "test.txt", "second commit");

        let args = HashMap::from([
            (
                "path".to_string(),
                temp_dir.path().to_string_lossy().to_string(),
            ),
            ("target".to_string(), "HEAD~1".to_string()),
        ]);

        let result = git_diff_tool(args).await;
        assert!(result.contains("diff --git"));
        assert!(result.contains("test.txt"));
        assert!(result.contains("-first commit"));
        assert!(result.contains("+second commit"));
    }
}
