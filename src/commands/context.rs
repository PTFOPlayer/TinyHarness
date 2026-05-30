use std::io::Write;

use tinyharness_lib::context::WorkspaceContext;
use tinyharness_ui::output::Output;

use crate::style::*;

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
