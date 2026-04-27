use crate::context::WorkspaceContext;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const GRAY: &str = "\x1b[90m";

pub fn execute(ctx: &WorkspaceContext) {
    println!("\n{}Workspace Context:{}", BOLD, RESET);
    println!("  {}Project:{}{} {} ({})", GRAY, RESET, BOLD, ctx.project_name, ctx.project_type);
    println!("  {}Root:{}{} {}", GRAY, RESET, BOLD, ctx.root.display());
    println!(
        "  {}Git repo:{}{} {}",
        GRAY,
        RESET,
        BOLD,
        if ctx.is_git_repo { "yes" } else { "no" }
    );

    if !ctx.build_command.is_empty() {
        println!("  {}Build:{}{} {}", GRAY, RESET, BOLD, ctx.build_command);
    }
    if !ctx.test_command.is_empty() {
        println!("  {}Test:{}{} {}", GRAY, RESET, BOLD, ctx.test_command);
    }

    println!("\n{}Structure:{}", BOLD, RESET);
    for entry in &ctx.structure {
        println!("  {}", entry);
    }
    println!();
}
