use std::io::Write;

use tinyharness_lib::context::WorkspaceContext;
use tinyharness_ui::output::Output;

use tinyharness_ui::style::*;

pub fn execute(out: &mut Output, ctx: &WorkspaceContext) {
    let _ = writeln!(out, "\n{BOLD}Workspace Context:{RESET}");
    let _ = writeln!(
        out,
        "  {GRAY}Project:{RESET} {BOLD}{}{RESET} ({})",
        ctx.project_name, ctx.project_type,
    );
    let _ = writeln!(
        out,
        "  {GRAY}Root:{RESET} {BOLD}{}{RESET}",
        ctx.root.display(),
    );
    let _ = writeln!(
        out,
        "  {GRAY}Git repo:{RESET} {BOLD}{}{RESET}",
        if ctx.is_git_repo { "yes" } else { "no" },
    );

    if !ctx.build_command.is_empty() {
        let _ = writeln!(
            out,
            "  {GRAY}Build:{RESET} {BOLD}{}{RESET}",
            ctx.build_command,
        );
    }
    if !ctx.test_command.is_empty() {
        let _ = writeln!(
            out,
            "  {GRAY}Test:{RESET} {BOLD}{}{RESET}",
            ctx.test_command,
        );
    }

    let _ = writeln!(out, "\n{BOLD}Structure:{RESET}");
    for entry in &ctx.structure {
        let _ = writeln!(out, "  {entry}");
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{captured_output, captured_string, strip_ansi};
    use std::path::PathBuf;
    use tinyharness_lib::context::WorkspaceContext;

    fn test_context() -> WorkspaceContext {
        WorkspaceContext {
            root: PathBuf::from("/tmp/test-project"),
            project_type: "Rust".to_string(),
            project_name: "test-project".to_string(),
            is_git_repo: true,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            structure: vec!["Cargo.toml".to_string(), "src/".to_string()],
            project_md: None,
            additional_project_mds: vec![],
        }
    }

    #[test]
    fn context_shows_project_info() {
        let (mut out, buf) = captured_output();
        execute(&mut out, &test_context());
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Workspace Context:"));
        assert!(plain.contains("test-project"));
        assert!(plain.contains("Rust"));
        assert!(plain.contains("/tmp/test-project"));
        assert!(plain.contains("yes")); // git repo
    }

    #[test]
    fn context_shows_build_and_test_commands() {
        let (mut out, buf) = captured_output();
        execute(&mut out, &test_context());
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("cargo build"));
        assert!(plain.contains("cargo test"));
    }

    #[test]
    fn context_no_git_repo() {
        let (mut out, buf) = captured_output();
        let ctx = WorkspaceContext {
            is_git_repo: false,
            ..test_context()
        };
        execute(&mut out, &ctx);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("no"));
    }

    #[test]
    fn context_shows_structure() {
        let (mut out, buf) = captured_output();
        execute(&mut out, &test_context());
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Cargo.toml"));
        assert!(plain.contains("src/"));
    }
}
