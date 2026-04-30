use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::provider::{ToolFunctionInfo, ToolInfo, ToolType};
use crate::tools::tool::{build_string_params_schema, sync_to_async, Tool};

pub fn run_tool(args: HashMap<String, String>) -> String {
    let command = match args.get("command") {
        Some(c) => c,
        None => return "Error: 'command' argument is required".to_string(),
    };

    let timeout_ms: u64 = args
        .get("timeout")
        .and_then(|t| t.parse().ok())
        .unwrap_or(30_000);

    let cwd = args.get("cwd").map(|s| s.as_str());

    let timeout = Duration::from_millis(timeout_ms);

    // Use shell to run the command
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.arg("/C");
        c.arg(command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c");
        c.arg(command);
        c
    };

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    // Start the command
    let mut child = match cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("Error: Failed to spawn command: {}", e),
    };

    let start = Instant::now();

    // Poll for completion with timeout
    loop {
        if start.elapsed() >= timeout {
            let _ = child.kill();
            // Wait for the process to actually die
            let _ = child.wait();
            return format!(
                "Error: Command timed out after {}ms\nCommand: {}\nConsider increasing the timeout or simplifying the command.",
                timeout_ms, command
            );
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child.stdout.take().map(|s| {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::BufReader::new(s).read_to_string(&mut buf).ok();
                    buf
                }).unwrap_or_default();

                let stderr = child.stderr.take().map(|s| {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::BufReader::new(s).read_to_string(&mut buf).ok();
                    buf
                }).unwrap_or_default();

                let elapsed = start.elapsed();

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

                return result;
            }
            Ok(None) => {
                // Still running, sleep briefly
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return format!("Error: Failed to wait for command: {}", e);
            }
        }
    }
}

pub fn run_tool_entry() -> Tool {
    let tool_info = ToolInfo {
        tool_type: ToolType::Function,
        function: ToolFunctionInfo {
            name: "run".to_string(),
            description: "Execute a shell command and return its output. Use for building, testing, running git commands, or any terminal operation. Includes stdout, stderr, exit code, and duration. Output is truncated at 5000 chars for stdout and 2000 for stderr. Default timeout is 30 seconds.".to_string(),
            parameters: build_string_params_schema(
                &[("command", "The shell command to execute")],
                &[
                    ("timeout", "Timeout in milliseconds (default: 30000)", "30000"),
                    ("cwd", "Working directory for the command (default: project root)", ""),
                ],
            ),
        },
    };

    Tool {
        name: "run".to_string(),
        function: sync_to_async(run_tool),
        tool_info,
    }
}
