use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// A shell command with template substitution and timeout.
///
/// The `command` field is a template string with `{var}` placeholders.
/// At execution time, the placeholders are replaced with values from a
/// `HashMap<String, String>`. The command is run via `sh -c` (or `cmd /C`
/// on Windows) with the substituted variables also available as `TH_*`
/// environment variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellCommand {
    /// Command template, e.g. `"docker run --rm {image} sh -c \"$TH_COMMAND\""`.
    pub command: String,
    /// Timeout in seconds. Default: 30.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Working directory for the command. If `None`, inherits the current
    /// directory of the TinyHarness process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

fn default_timeout() -> u64 {
    30
}

impl ShellCommand {
    /// Shell-escape a value for safe inline substitution into a shell command.
    ///
    /// Wraps the value in single quotes and escapes any single quotes within
    /// it by replacing `'` with `'\''`. This prevents shell metacharacters
    /// in the value (e.g. from LLM responses) from breaking the command.
    fn shell_escape(value: &str) -> String {
        // Replace ' with '\'' and wrap in single quotes.
        // e.g.  I'll do it  →  'I'\''ll do it'
        let escaped = value.replace('\'', "'\\''");
        format!("'{}'", escaped)
    }

    /// Render the command template by substituting `{var}` placeholders.
    ///
    /// Variable values are shell-escaped (wrapped in single quotes) to
    /// prevent shell injection from untrusted content like LLM responses.
    ///
    /// Missing variables (not present in `vars`) are left as-is — the
    /// `{placeholder}` appears literally in the rendered command.
    pub fn render(&self, vars: &HashMap<String, String>) -> String {
        let mut result = self.command.clone();
        for (key, value) in vars {
            let placeholder = format!("{{{}}}", key);
            let escaped = Self::shell_escape(value);
            result = result.replace(&placeholder, &escaped);
        }
        result
    }

    /// Execute the command asynchronously.
    ///
    /// Returns `Ok(stdout)` on success (exit code 0), or `Err(message)` on
    /// failure (non-zero exit, timeout, or spawn error).
    ///
    /// The `vars` are used for both `{var}` template substitution and as
    /// `TH_*` environment variables.
    pub async fn execute(&self, vars: &HashMap<String, String>) -> Result<String, String> {
        let rendered = self.render(vars);

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&rendered);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(&rendered);
            c
        };

        // Set environment variables
        for (key, value) in vars {
            let env_key = format!("TH_{}", key.to_uppercase().replace([' ', '-'], "_"));
            cmd.env(env_key, value);
        }
        // Also set the raw key as an env var for convenience
        for (key, value) in vars {
            cmd.env(key, value);
        }

        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        let timeout = tokio::time::Duration::from_secs(self.timeout_secs);
        let wait_result = tokio::time::timeout(timeout, child.wait()).await;

        match wait_result {
            Ok(Ok(status)) => {
                let stdout = read_pipe(child.stdout.take()).await;
                let stderr = read_pipe(child.stderr.take()).await;

                if status.success() {
                    Ok(stdout.trim().to_string())
                } else {
                    let mut msg =
                        format!("Command exited with code {}", status.code().unwrap_or(-1));
                    if !stderr.is_empty() {
                        msg.push_str(&format!("\n{}", stderr.trim()));
                    }
                    if !stdout.is_empty() {
                        msg.push_str(&format!("\n{}", stdout.trim()));
                    }
                    Err(msg)
                }
            }
            Ok(Err(e)) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(format!("Failed to wait for command: {}", e))
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(format!(
                    "Command timed out after {}s: {}",
                    self.timeout_secs, rendered
                ))
            }
        }
    }
}

/// Read a piped stdout/stderr to a string, truncating at 10 KB.
async fn read_pipe<T: tokio::io::AsyncRead + Unpin>(mut pipe: Option<T>) -> String {
    if let Some(ref mut r) = pipe.as_mut() {
        let mut buf = String::new();
        let _ = r.read_to_string(&mut buf).await;
        // Truncate at 10 KB to avoid huge outputs
        let max_chars = 10_000;
        if buf.chars().count() > max_chars {
            let truncated: String = buf.chars().take(max_chars).collect();
            format!("{}\n... (truncated)", truncated)
        } else {
            buf
        }
    } else {
        String::new()
    }
}

/// Execute a shell command for a custom tool, substituting tool arguments.
///
/// This is used by custom tool handlers. The tool arguments are passed as a
/// `HashMap<String, String>` (same as built-in tools).
pub async fn execute_shell_tool(command: &ShellCommand, args: &HashMap<String, String>) -> String {
    match command.execute(args).await {
        Ok(stdout) => {
            if stdout.is_empty() {
                "(no output)".to_string()
            } else {
                stdout
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_basic_substitution() {
        let cmd = ShellCommand {
            command: "echo {name}".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "world".to_string());
        // Values are shell-escaped (wrapped in single quotes)
        assert_eq!(cmd.render(&vars), "echo 'world'");
    }

    #[test]
    fn test_render_multiple_vars() {
        let cmd = ShellCommand {
            command: "docker run --rm {image} sh -c {command}".to_string(),
            timeout_secs: 30,
            cwd: None,
        };
        let mut vars = HashMap::new();
        vars.insert("image".to_string(), "ubuntu:latest".to_string());
        vars.insert("command".to_string(), "ls -la".to_string());
        // Each value is individually shell-escaped
        assert_eq!(
            cmd.render(&vars),
            "docker run --rm 'ubuntu:latest' sh -c 'ls -la'"
        );
    }

    #[test]
    fn test_render_shell_escapes_single_quotes() {
        let cmd = ShellCommand {
            command: "echo {text}".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let mut vars = HashMap::new();
        vars.insert("text".to_string(), "I'll do it".to_string());
        // Single quotes in the value are escaped: ' -> '\''
        assert_eq!(cmd.render(&vars), "echo 'I'\\''ll do it'");
    }

    #[test]
    fn test_render_shell_escapes_metacharacters() {
        let cmd = ShellCommand {
            command: "echo -n {response} | wc -c".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let mut vars = HashMap::new();
        vars.insert(
            "response".to_string(),
            "I'll find all files in `./src` and run $(whoami)".to_string(),
        );
        let rendered = cmd.render(&vars);
        // The value is safely quoted — metacharacters are inert
        assert_eq!(
            rendered,
            "echo -n 'I'\\''ll find all files in `./src` and run $(whoami)' | wc -c"
        );
    }

    #[test]
    fn test_render_missing_var_stays_as_placeholder() {
        let cmd = ShellCommand {
            command: "echo {missing}".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let vars = HashMap::new();
        // Missing vars are left as-is (not replaced with empty string)
        assert_eq!(cmd.render(&vars), "echo {missing}");
    }

    #[test]
    fn test_render_no_vars() {
        let cmd = ShellCommand {
            command: "git status".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let vars = HashMap::new();
        assert_eq!(cmd.render(&vars), "git status");
    }

    #[test]
    fn test_default_timeout() {
        assert_eq!(default_timeout(), 30);
    }

    #[test]
    fn test_shell_command_serde() {
        let json = r#"{"command": "echo hi", "timeout_secs": 10}"#;
        let cmd: ShellCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.command, "echo hi");
        assert_eq!(cmd.timeout_secs, 10);
        assert!(cmd.cwd.is_none());
    }

    #[test]
    fn test_shell_command_serde_default_timeout() {
        let json = r#"{"command": "echo hi"}"#;
        let cmd: ShellCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.timeout_secs, 30);
    }

    #[test]
    fn test_shell_command_serde_with_cwd() {
        let json = r#"{"command": "ls", "timeout_secs": 5, "cwd": "/tmp"}"#;
        let cmd: ShellCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.cwd.as_deref(), Some("/tmp"));
    }

    #[tokio::test]
    async fn test_execute_success() {
        let cmd = ShellCommand {
            command: "echo hello".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let vars = HashMap::new();
        let result = cmd.execute(&vars).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_execute_with_substitution() {
        let cmd = ShellCommand {
            command: "echo {name}".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "world".to_string());
        let result = cmd.execute(&vars).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "world");
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let cmd = ShellCommand {
            command: "exit 1".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let vars = HashMap::new();
        let result = cmd.execute(&vars).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exited with code 1"));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let cmd = ShellCommand {
            command: (if cfg!(target_os = "windows") {
                "timeout /T 100 /NOBREAK > NUL"
            } else {
                "sleep 100"
            })
            .to_string(),
            timeout_secs: 1,
            cwd: None,
        };
        let vars = HashMap::new();
        let result = cmd.execute(&vars).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_env_var_available() {
        // The vars should be available as TH_* env vars
        let cmd = ShellCommand {
            command: (if cfg!(target_os = "windows") {
                "echo %TH_NAME%"
            } else {
                "echo $TH_NAME"
            })
            .to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "from_env".to_string());
        let result = cmd.execute(&vars).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "from_env");
    }

    #[tokio::test]
    async fn test_execute_cwd() {
        let tmp = std::env::temp_dir();
        let cmd = ShellCommand {
            command: "pwd".to_string(),
            timeout_secs: 5,
            cwd: Some(tmp.to_string_lossy().to_string()),
        };
        let vars = HashMap::new();
        let result = cmd.execute(&vars).await;
        assert!(result.is_ok());
        // On macOS /tmp is a symlink to /private/tmp
        let expected = std::fs::canonicalize(&tmp)
            .unwrap_or_else(|_| tmp.clone())
            .to_string_lossy()
            .to_string();
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn test_execute_shell_tool_success() {
        let cmd = ShellCommand {
            command: "echo hello {name}".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let mut args = HashMap::new();
        args.insert("name".to_string(), "world".to_string());
        let result = execute_shell_tool(&cmd, &args).await;
        assert_eq!(result, "hello world");
    }

    #[tokio::test]
    async fn test_execute_shell_tool_error() {
        let cmd = ShellCommand {
            command: "exit 42".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let args = HashMap::new();
        let result = execute_shell_tool(&cmd, &args).await;
        assert!(result.starts_with("Error:"));
    }

    #[tokio::test]
    async fn test_execute_shell_tool_empty_output() {
        let cmd = ShellCommand {
            command: "true".to_string(),
            timeout_secs: 5,
            cwd: None,
        };
        let args = HashMap::new();
        let result = execute_shell_tool(&cmd, &args).await;
        assert_eq!(result, "(no output)");
    }
}
