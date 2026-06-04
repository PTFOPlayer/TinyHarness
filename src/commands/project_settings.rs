//! `/project-settings` — view and init per-project settings.
//!
//! Per-project settings live in `.tinyharness/config.json` (discovered by walking
//! up from CWD) and override/merge with the global `~/.config/tinyharness/settings.json`.

use std::io::Write;

use tinyharness_lib::config::{
    SettingSource, discover_project_settings, load_merged_settings, load_settings,
};
use tinyharness_ui::output::Output;
use tinyharness_ui::style::*;

use crate::commands::registry::CommandResult;

pub fn execute(out: &mut Output, arg: Option<&str>) -> Result<CommandResult, String> {
    match arg {
        Some("init") => execute_init(out),
        _ => execute_show(out),
    }
    Ok(CommandResult::Ok)
}

/// Show merged effective settings with source annotations.
fn execute_show(out: &mut Output) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project_config = discover_project_settings(&cwd);

    let has_project = matches!(&project_config, Some(Ok(_)));

    let (_, _, merged) = load_merged_settings();

    let _ = writeln!(out);
    if has_project {
        let _ = writeln!(
            out,
            "{BOLD}╭─ Project Settings (.tinyharness/config.json) ─╮{RESET}",
        );
    } else {
        let _ = writeln!(
            out,
            "{BOLD}╭─ Project Settings (no .tinyharness/config.json) ─╮{RESET}",
        );
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} {GRAY}Use {BOLD}/project-settings init{RESET}{GRAY} to create one.{RESET}",
        );
    }

    // ── Preferred mode ──
    let (mode_str, mode_src) = format_setting(
        &merged.preferred_mode.to_string(),
        merged.preferred_mode_source,
        Some("default: casual"),
    );
    let _ = writeln!(out, "{BOLD}│{RESET} Mode:      {mode_str} {mode_src}");

    // ── Auto-accept ──
    let aa_val = if merged.auto_accept_safe_commands {
        "enabled"
    } else {
        "disabled"
    };
    let (aa_str, aa_src) = format_setting(aa_val, merged.auto_accept_source, None);
    let _ = writeln!(out, "{BOLD}│{RESET} Auto-Accept: {aa_str} {aa_src}");

    // ── Context limit ──
    let ctx_val = merged
        .context_limit
        .map(|n| format!("{} tokens", n))
        .unwrap_or_else(|| "auto".to_string());
    let (ctx_str, ctx_src) = format_setting(&ctx_val, merged.context_limit_source, None);
    let _ = writeln!(out, "{BOLD}│{RESET} Ctx Limit:  {ctx_str} {ctx_src}");

    // ── Safe commands ──
    let safe_count = merged.safe_commands.len();
    let (safe_str, safe_src) = format_setting(
        &format!("{} configured", safe_count),
        merged.safe_commands_source,
        None,
    );
    let _ = writeln!(out, "{BOLD}│{RESET} Safe Cmds:  {safe_str} {safe_src}");

    // ── Denied commands ──
    if !merged.denied_commands.is_empty() {
        let denied_count = merged.denied_commands.len();
        let (denied_str, denied_src) = format_setting(
            &format!("{} denied", denied_count),
            merged.denied_commands_source,
            None,
        );
        let _ = writeln!(out, "{BOLD}│{RESET} Denied:     {denied_str} {denied_src}");
    }

    // ── Additional MD files ──
    if !merged.project_md_files.is_empty() {
        let files_str = merged.project_md_files.join(", ");
        let (md_str, md_src) = format_setting(&files_str, merged.project_md_files_source, None);
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} Extra MD:   {BLUE}{md_str}{RESET} {md_src}"
        );
    }

    let _ = writeln!(
        out,
        "{BOLD}╰─────────────────────────────────────────────────╯{RESET}",
    );

    // ── Legend ──
    let _ = writeln!(
        out,
        "\n{GRAY}Legend: {GREEN}(project){GRAY} = from .tinyharness/config.json, {CYAN}(global){GRAY} = from ~/.config/tinyharness/settings.json, {ORANGE}(default){GRAY} = hardcoded default{RESET}",
    );

    // ── Show project config path if found ──
    if let Some(Ok(_)) = &project_config {
        let mut dir = cwd;
        loop {
            let candidate = dir.join(".tinyharness").join("config.json");
            if candidate.is_file() {
                let _ = writeln!(
                    out,
                    "\n{GRAY}Config file: {BLUE}{}{RESET}",
                    candidate.display()
                );
                break;
            }
            if let Some(parent) = dir.parent() {
                if parent == dir {
                    break;
                }
                dir = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    let _ = writeln!(out);
}

/// Generate a `.tinyharness/config.json` template from current settings.
fn execute_init(out: &mut Output) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dir = cwd.join(".tinyharness");

    // Check if config already exists
    let config_path = dir.join("config.json");
    if config_path.exists() {
        let _ = writeln!(
            out,
            "{ORANGE}⚠ .tinyharness/config.json already exists at {BLUE}{}{RESET}",
            config_path.display()
        );
        let _ = writeln!(
            out,
            "{GRAY}  Delete it first if you want to regenerate: rm {}{}{RESET}",
            BLUE,
            config_path.display(),
        );
        return;
    }

    // Create the .tinyharness directory
    if let Err(e) = std::fs::create_dir_all(&dir) {
        let _ = writeln!(out, "{RED}Failed to create directory: {e}{RESET}");
        return;
    }

    // Generate template from current settings
    let _settings = load_settings();

    // Write the template (with empty project_md_files as an example)
    let json = r#"{
  // Safe command prefixes: these extend the global safe list.
  // Add project-specific commands that should be auto-accepted.
  // "safe_command_prefixes": ["python -m pytest", "npm run lint"],

  // Denied command prefixes: always block these, even if they'd match a safe prefix.
  // "denied_command_prefixes": ["git push --force"],

  // Whether to auto-accept safe read-only commands.
  "auto_accept_safe_commands": true,

  // Context limit in tokens. Set to null for model default.
  // "context_limit": null,

  // Additional project MD files to include in the AI's context.
  // These are loaded AFTER TINYHARNESS.md or AGENTS.md.
  // "project_md_files": ["RULES.md", ".cursorrules"],

  // Preferred agent mode for this project.
  // Valid modes: casual, planning, agent, research
  // "preferred_mode": "agent"
}
"#;

    // Strip comments for valid JSON output
    let stripped = strip_json_comments(json);

    if let Err(e) = std::fs::write(&config_path, &stripped) {
        let _ = writeln!(out, "{RED}Failed to write config file: {e}{RESET}");
        return;
    }

    let _ = writeln!(
        out,
        "\n{GREEN}✔ Created {BLUE}{}{GREEN}{RESET}",
        config_path.display()
    );
    let _ = writeln!(
        out,
        "{GRAY}Edit this file to customize per-project settings.{RESET}",
    );
    let _ = writeln!(
        out,
        "{GRAY}Uncomment and change fields as needed. Run {BOLD}/project-settings{RESET}{GRAY} to view merged settings.{RESET}",
    );
    let _ = writeln!(out);
}

/// Naive JSON comment stripper: removes `// ...` line comments and `/* ... */` blocks.
fn strip_json_comments(json: &str) -> String {
    let mut result = String::new();
    let mut chars = json.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch == '/' {
            chars.next(); // consume first /
            match chars.peek() {
                Some(&'/') => {
                    // Line comment: skip until end of line
                    chars.next();
                    while let Some(&c) = chars.peek() {
                        chars.next();
                        if c == '\n' {
                            // Keep the newline
                            result.push('\n');
                            break;
                        }
                    }
                }
                Some(&'*') => {
                    // Block comment: skip until */
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('*') => {
                                if let Some(&'/') = chars.peek() {
                                    chars.next();
                                    break;
                                }
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                _ => {
                    result.push('/');
                }
            }
        } else {
            result.push(ch);
            chars.next();
        }
    }

    result
}

/// Format a setting value with a source annotation tag.
fn format_setting(value: &str, source: SettingSource, extra: Option<&str>) -> (String, String) {
    let src_tag = match source {
        SettingSource::Project => format!("{GREEN}(project){RESET}"),
        SettingSource::Global => format!("{CYAN}(global){RESET}"),
        SettingSource::Default => format!("{ORANGE}(default){RESET}"),
    };

    let display = if let Some(extra_info) = extra {
        format!("{BLUE}{value}{RESET} {GRAY}{extra_info}{RESET}")
    } else {
        format!("{BLUE}{value}{RESET}")
    };

    (display, src_tag)
}
