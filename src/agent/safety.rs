/// Strip shell descriptor redirections that are considered safe:
/// - `2>&1`  — redirect stderr to stdout
/// - `N>/dev/null` — redirect a file descriptor to /dev/null
///
/// These don't write to arbitrary files so they're safe for auto-accept.
pub fn strip_safe_descriptor_redirections(command: &str) -> String {
    // Remove `2>&1` (and `1>&2`, `0>&1`, etc. — any single-digit descriptor redirect)
    let re = regex::Regex::new(r"\d>&\d").unwrap();
    let result = re.replace_all(command, "");
    // Remove `N>/dev/null` patterns (e.g. `2>/dev/null`, `2> /dev/null`)
    let re = regex::Regex::new(r"\d>\s*/dev/null").unwrap();
    re.replace_all(&result, "").to_string()
}

/// Check if a shell command is safe to auto-accept (read-only operations).
/// Returns `true` if the command only uses safe, read-only utilities AND
/// does not match any prefix in the denied list.
pub fn is_safe_command(
    command: &str,
    safe_commands: &[String],
    denied_commands: &[String],
) -> bool {
    let command = command.trim();

    // Accept empty commands
    if command.is_empty() {
        return true;
    }

    // Check the denied list first — it takes priority over the safe list.
    // If a command matches a denied prefix, it is always blocked from auto-accept.
    for denied in denied_commands {
        if command.starts_with(denied) {
            let rest = &command[denied.len()..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('=') {
                return false;
            }
        }
    }

    // Reject commands with newlines — could hide a second command
    if command.contains('\n') {
        return false;
    }

    // Reject commands with semicolons — `cd /path ; rm -rf /` bypasses safety
    if command.contains(';') {
        return false;
    }

    // Strip safe descriptor redirections early so they don't trigger
    // false positives in later checks:
    //   - `2>&1` contains `&` (would be flagged as background operator)
    //   - `2>/dev/null` contains `>` (would be flagged as file redirection)
    // These patterns are safe — they only redirect stderr, not arbitrary files.
    let stripped = strip_safe_descriptor_redirections(command);

    // Reject commands with single `&` (background operator) — `sleep 1 & rm -rf /`
    // `&&` is a logical AND and is handled separately below.
    // Check for any `&` that is not part of `&&`.
    let bytes = stripped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            // Check if this is part of `&&`
            if i + 1 < bytes.len() && bytes[i + 1] == b'&' {
                i += 2; // skip the `&&`
            } else {
                return false; // lone `&` — background operator
            }
        } else {
            i += 1;
        }
    }

    // Handle `&&` and `||` chaining: models often write `cd /path && ls`
    // Split on `&&` / `||` and check that every part is individually safe.
    // This must come before the `|` check since `||` contains `|`.
    if stripped.contains("&&") || stripped.contains("||") {
        let parts: Vec<&str> = stripped
            .split("&&")
            .flat_map(|part| part.split("||"))
            .collect();
        return parts
            .iter()
            .all(|part| is_safe_command(part, safe_commands, denied_commands));
    }

    // Reject commands with pipes, command substitution, or remaining redirections.
    // Safe descriptor redirections (2>&1, 2>/dev/null) have already been stripped.
    if stripped.contains('|') || stripped.contains("$(") || stripped.contains("`") {
        return false;
    }

    if stripped.contains('>') || stripped.contains('<') {
        return false;
    }

    // Check if command starts with any safe prefix followed by a word boundary
    // (space, end-of-string, or `=`). This prevents `cdx` from matching `cd`.
    // Use the ORIGINAL command (not stripped) for prefix matching, since the
    // stripped version has safe redirections removed.
    for safe_cmd in safe_commands {
        if command.starts_with(safe_cmd) {
            let rest = &command[safe_cmd.len()..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('=') {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_ls_error_passthrough() {
        // This test was moved from agent.rs but tests the display module
        // Keeping it here as a sanity check for the safety module
    }

    #[test]
    fn test_is_safe_command_basic() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command("ls", &safe_commands, &[]));
        assert!(is_safe_command("ls -la", &safe_commands, &[]));
        assert!(is_safe_command("grep foo bar.txt", &safe_commands, &[]));
        assert!(is_safe_command("find . -name '*.rs'", &safe_commands, &[]));
        assert!(is_safe_command("cat README.md", &safe_commands, &[]));
        assert!(is_safe_command("pwd", &safe_commands, &[]));
        assert!(is_safe_command("git status", &safe_commands, &[]));
        assert!(is_safe_command("git diff", &safe_commands, &[]));
        assert!(is_safe_command("cargo tree", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_unsafe() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(!is_safe_command("rm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command(
            "echo hello > file.txt",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("cat file | grep foo", &safe_commands, &[]));
        assert!(!is_safe_command("$(whoami)", &safe_commands, &[]));
        assert!(!is_safe_command("echo `whoami`", &safe_commands, &[]));
        assert!(!is_safe_command("cargo build", &safe_commands, &[]));
        assert!(!is_safe_command("cargo test", &safe_commands, &[]));
        assert!(!is_safe_command("git commit", &safe_commands, &[]));
        assert!(!is_safe_command("git push", &safe_commands, &[]));
        assert!(!is_safe_command("cargo run", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_with_whitespace() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command("  ls  ", &safe_commands, &[]));
        assert!(is_safe_command("\tgrep foo\n", &safe_commands, &[]));
        assert!(!is_safe_command("  rm -rf  ", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_custom_list() {
        let custom_commands = vec!["ls".to_string(), "custom-cmd".to_string()];
        assert!(is_safe_command("ls", &custom_commands, &[]));
        assert!(is_safe_command("custom-cmd arg", &custom_commands, &[]));
        assert!(!is_safe_command("grep foo", &custom_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_with_chain() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // cd && safe-command should be safe
        assert!(is_safe_command("cd /some/path && ls", &safe_commands, &[]));
        assert!(is_safe_command(
            "cd /project && git status",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command(
            "cd /project && git diff",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command("pwd && ls", &safe_commands, &[]));
        // All parts must be safe
        assert!(!is_safe_command(
            "cd /path && rm -rf /",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("ls && cargo build", &safe_commands, &[]));
        assert!(!is_safe_command("ls && git push", &safe_commands, &[]));
        // Pipe inside a chained part is still rejected
        assert!(!is_safe_command(
            "cd /path && cat file | grep foo",
            &safe_commands,
            &[]
        ));
        // Redirection inside a chained part is still rejected
        assert!(!is_safe_command(
            "cd /path && echo hello > file.txt",
            &safe_commands,
            &[]
        ));
    }

    #[test]
    fn test_is_safe_command_semicolon_separator() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(!is_safe_command("cd /path ; rm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command("ls ; pwd", &safe_commands, &[]));
        assert!(!is_safe_command("ls;rm -rf /", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_background_ampersand() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(!is_safe_command("sleep 1 & rm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command("ls & pwd", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_newline_separator() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(!is_safe_command("cd /path\nrm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command("ls\npwd", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_or_chain() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command("cd /path || ls", &safe_commands, &[]));
        assert!(is_safe_command("pwd || ls", &safe_commands, &[]));
        assert!(!is_safe_command(
            "cd /path || rm -rf /",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("ls || cargo build", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_mixed_chains() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command(
            "cd /path && ls || pwd",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command(
            "cd /path && ls || rm -rf /",
            &safe_commands,
            &[]
        ));
    }

    #[test]
    fn test_is_safe_command_word_boundary() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command("ls", &safe_commands, &[]));
        assert!(is_safe_command("ls -la", &safe_commands, &[]));
        assert!(!is_safe_command("lsx", &safe_commands, &[]));
        assert!(!is_safe_command("cdx", &safe_commands, &[]));
        assert!(!is_safe_command("catt", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_descriptor_redirection() {
        let mut safe_commands = tinyharness_lib::config::get_default_safe_commands();
        safe_commands.push("cargo test".to_string());

        assert!(is_safe_command("cargo test 2>&1", &safe_commands, &[]));
        assert!(is_safe_command("ls -la 2>&1", &safe_commands, &[]));
        assert!(is_safe_command(
            "cargo test 2>/dev/null",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command(
            "find . -name '*.rs' 2>/dev/null",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command("ls 2> /dev/null", &safe_commands, &[]));
        assert!(is_safe_command(
            "cd /path && cargo test 2>&1",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command(
            "echo hello > file.txt",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command(
            "cat file > output.txt",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("cat < input.txt", &safe_commands, &[]));
    }

    #[test]
    fn test_strip_safe_descriptor_redirections() {
        assert_eq!(strip_safe_descriptor_redirections("ls 2>&1"), "ls ");
        assert_eq!(
            strip_safe_descriptor_redirections("cmd 2>&1 1>/dev/null"),
            "cmd  "
        );
        assert_eq!(
            strip_safe_descriptor_redirections("find . 2>/dev/null"),
            "find . "
        );
        assert_eq!(
            strip_safe_descriptor_redirections("find . 2> /dev/null"),
            "find . "
        );
        assert_eq!(
            strip_safe_descriptor_redirections("echo hello > file.txt"),
            "echo hello > file.txt"
        );
    }

    #[test]
    fn test_is_safe_command_denied_list() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();

        assert!(is_safe_command("git status", &safe_commands, &[]));

        let denied = vec!["git push".to_string()];
        assert!(is_safe_command("git status", &safe_commands, &denied));
        assert!(!is_safe_command("git push", &safe_commands, &denied));
        assert!(!is_safe_command(
            "git push origin main",
            &safe_commands,
            &denied
        ));

        let denied_cargo = vec!["cargo".to_string()];
        assert!(!is_safe_command(
            "cargo build",
            &safe_commands,
            &denied_cargo
        ));
        assert!(!is_safe_command(
            "cargo test",
            &safe_commands,
            &denied_cargo
        ));
        assert!(!is_safe_command(
            "cargo tree",
            &safe_commands,
            &denied_cargo
        ));

        let denied_echo = vec!["echo".to_string()];
        assert!(!is_safe_command("echo hello", &safe_commands, &denied_echo));

        let denied_ps = vec!["ps".to_string()];
        assert!(!is_safe_command("ps", &safe_commands, &denied_ps));
        assert!(!is_safe_command("ps aux", &safe_commands, &denied_ps));
        assert!(!is_safe_command("psx", &safe_commands, &denied_ps));

        let denied_git_push = vec!["git push".to_string()];
        assert!(!is_safe_command(
            "cd /path && git push",
            &safe_commands,
            &denied_git_push
        ));
        assert!(is_safe_command(
            "cd /path && git status",
            &safe_commands,
            &denied_git_push
        ));
    }
}
