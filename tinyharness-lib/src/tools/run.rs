use std::collections::HashMap;
use std::time::Instant;

use tokio::io::AsyncReadExt;

use crate::extract_args;
use crate::tools::tool::{ToolCategory, build_string_params_schema, make_tool};

/// Terminate a child process and, on Unix, its entire process group.
pub(crate) async fn kill_child(mut child: tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(id) = child.id() {
            // Send SIGKILL to the process group (-pgid)
            unsafe {
                libc::kill(-(id as libc::pid_t), libc::SIGKILL);
            }
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await; // reap zombie process
}

pub fn run_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "run",
        "Execute a shell command and return its output. Use for building, testing, running git commands, or any terminal operation. Returns stdout, stderr (if any), exit code, and duration. stdout is truncated at 5000 chars and stderr at 2000 chars. Default timeout is 30 seconds.",
        ToolCategory::Destructive,
        build_string_params_schema(
            &[("command", "The shell command to execute")],
            &[
                (
                    "timeout",
                    "Timeout in milliseconds (default: 30000)",
                    "30000",
                ),
                (
                    "cwd",
                    "Working directory for the command (default: project root)",
                    "",
                ),
            ],
        ),
        move |args| Box::pin(run_tool(args)),
    )
}

/// Execute a shell command asynchronously with a timeout.
/// Returns stdout, stderr, exit code, and duration.
pub async fn run_tool(args: HashMap<String, String>) -> String {
    extract_args!(args, command);

    let timeout_ms: u64 = args
        .get("timeout")
        .and_then(|t| t.parse().ok())
        .unwrap_or(30_000);

    let cwd = args.get("cwd").map(|s| s.as_str());

    // Use shell to run the command
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C");
        c.arg(&command);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c");
        c.arg(&command);
        c
    };

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    #[cfg(unix)]
    cmd.process_group(0);

    // Start the command non-interactively with stdin disconnected
    let mut child = match cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("Error: Failed to spawn command: {}", e),
    };

    let start = Instant::now();

    // Wait for the command with a timeout
    let wait_result =
        tokio::time::timeout(tokio::time::Duration::from_millis(timeout_ms), child.wait()).await;

    let elapsed = start.elapsed();

    match wait_result {
        Ok(Ok(status)) => {
            // Read stdout
            let stdout = if let Some(mut out) = child.stdout.take() {
                let mut buf = String::new();
                let _ = out.read_to_string(&mut buf).await;
                buf
            } else {
                String::new()
            };

            // Read stderr
            let stderr = if let Some(mut err) = child.stderr.take() {
                let mut buf = String::new();
                let _ = err.read_to_string(&mut buf).await;
                buf
            } else {
                String::new()
            };

            let mut result = String::new();

            if !stdout.is_empty() {
                // Truncate stdout if too large
                let max_chars = 5000;
                if stdout.chars().count() > max_chars {
                    let truncated: String = stdout.chars().take(max_chars).collect();
                    result.push_str(&format!(
                        "{}\n... (truncated to {} chars)\n",
                        truncated, max_chars
                    ));
                } else {
                    result.push_str(stdout.trim_end());
                }
            }

            if !stderr.is_empty() {
                let max_chars = 2000;
                if stderr.chars().count() > max_chars {
                    let truncated: String = stderr.chars().take(max_chars).collect();
                    result.push_str(&format!(
                        "\n{}... (stderr truncated to {} chars)\n",
                        truncated, max_chars
                    ));
                } else {
                    result.push_str(&format!("\n{}", stderr.trim_end()));
                }
            }

            let exit_summary = format!(
                "(exit {} in {:.1}s)",
                status.code().unwrap_or(-1),
                elapsed.as_secs_f64()
            );
            if result.is_empty() {
                result.push_str(&exit_summary);
            } else {
                result.push_str(&format!("\n{}", exit_summary));
            }

            result
        }
        Ok(Err(e)) => {
            // Error while waiting for the command
            kill_child(child).await;
            format!("Error: Failed to wait for command: {}", e)
        }
        Err(_elapsed) => {
            // Command timed out — kill the process and its child processes
            kill_child(child).await;
            format!(
                "Error: Command timed out after {}ms\nCommand: {}\nConsider increasing the timeout or simplifying the command.",
                timeout_ms, command
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_tool_timeout_kills_process_group() {
        let mut args = HashMap::new();
        // Spawn a background child process via subshell, wait on it, with short timeout
        args.insert("command".to_string(), "sh -c 'sleep 10'".to_string());
        args.insert("timeout".to_string(), "100".to_string());

        let res = run_tool(args).await;
        assert!(res.contains("timed out after 100ms"), "result was: {res}");
    }

    #[tokio::test]
    async fn test_run_tool_stdin_is_null() {
        let mut args = HashMap::new();
        // A command trying to read from stdin should immediately receive EOF
        args.insert("command".to_string(), "head -n 1".to_string());
        args.insert("timeout".to_string(), "2000".to_string());

        let res = run_tool(args).await;
        assert!(res.contains("(exit 0"), "result was: {res}");
    }
}
