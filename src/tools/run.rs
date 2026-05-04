use std::collections::HashMap;
use std::time::Instant;

use tokio::io::AsyncReadExt;

use crate::tools::tool::{Tool, build_string_params_schema, make_tool, require_arg};

/// Execute a shell command asynchronously with a timeout.
/// Returns stdout, stderr, exit code, and duration.
pub async fn run_tool(args: HashMap<String, String>) -> String {
    let command = match require_arg(&args, "command") {
        Ok(c) => c,
        Err(e) => return e,
    };

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

    // Start the command
    let mut child = match cmd
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
                        "stdout (truncated to {} chars):\n{}\n... (output truncated)\n",
                        max_chars, truncated
                    ));
                } else {
                    result.push_str(&format!("stdout:\n{}\n", stdout.trim_end()));
                }
            }

            if !stderr.is_empty() {
                let max_chars = 2000;
                if stderr.chars().count() > max_chars {
                    let truncated: String = stderr.chars().take(max_chars).collect();
                    result.push_str(&format!(
                        "stderr (truncated to {} chars):\n{}\n... (stderr truncated)\n",
                        max_chars, truncated
                    ));
                } else {
                    result.push_str(&format!("stderr:\n{}\n", stderr.trim_end()));
                }
            }

            if status.success() {
                result.push_str(&format!(
                    "\nCommand completed successfully in {:.1}s (exit code: {})",
                    elapsed.as_secs_f64(),
                    status.code().unwrap_or(-1)
                ));
            } else {
                result.push_str(&format!(
                    "\nCommand failed (exit code: {}) in {:.1}s",
                    status.code().unwrap_or(-1),
                    elapsed.as_secs_f64()
                ));
            }

            result
        }
        Ok(Err(e)) => {
            // Error while waiting for the command
            let _ = child.kill().await;
            let _ = child.wait().await;
            format!("Error: Failed to wait for command: {}", e)
        }
        Err(_elapsed) => {
            // Command timed out — kill it
            let _ = child.kill().await;
            let _ = child.wait().await; // reap zombie process
            format!(
                "Error: Command timed out after {}ms\nCommand: {}\nConsider increasing the timeout or simplifying the command.",
                timeout_ms, command
            )
        }
    }
}

pub fn run_tool_entry() -> Tool {
    make_tool(
        "run",
        "Execute a shell command and return its output. Use for building, testing, running git commands, or any terminal operation. Includes stdout, stderr, exit code, and duration. Output is truncated at 5000 chars for stdout and 2000 for stderr. Default timeout is 30 seconds.",
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
