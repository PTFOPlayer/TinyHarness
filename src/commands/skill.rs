use std::io::Write;

use tinyharness_lib::skill::{SkillRegistry, SkillSource};

use crate::style::*;

/// List all available skills, marking active ones.
pub fn execute_list(registry: &SkillRegistry, active_skills: &[String]) {
    if registry.skills.is_empty() {
        println!();
        println!("{}No skills found.{}", ORANGE, RESET);
        println!(
            "{}Create skills in ~/.tinyharness/skills/<name>/SKILL.md{}",
            GRAY, RESET
        );
        println!(
            "{}or in .tinyharness/skills/<name>/SKILL.md (project-local){}",
            GRAY, RESET
        );
        println!();
        return;
    }

    println!();
    println!(
        "{}╭─ Available Skills ───────────────────────────╮{}",
        BOLD, RESET
    );

    for skill in &registry.skills {
        let source_label = match skill.source {
            SkillSource::Personal => format!("{}~{}", GRAY, RESET),
            SkillSource::Project => format!("{}.{}", GRAY, RESET),
        };
        let auto_label = if skill.disable_model_invocation {
            format!("{}manual only{}", GRAY, RESET)
        } else {
            format!("{}auto{}", GREEN, RESET)
        };
        let active_marker = if active_skills
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&skill.name))
        {
            format!("{}●{}", GREEN, RESET)
        } else {
            format!("{}○{}", GRAY, RESET)
        };

        println!(
            "{}│{} {} {}{}{}{} — {}  {}[{}]{}",
            BOLD,
            RESET,
            active_marker,
            BOLD,
            CYAN,
            skill.name,
            RESET,
            skill.description,
            source_label,
            auto_label,
            RESET
        );
    }

    println!(
        "{}╰──────────────────────────────────────────────╯{}",
        BOLD, RESET
    );

    if !active_skills.is_empty() {
        println!(
            "  {}Active: {}{}{}",
            GRAY,
            GREEN,
            active_skills.join(", "),
            RESET
        );
    }

    println!();
}

/// Show details for a specific skill.
pub fn execute_show<W: Write>(
    registry: &SkillRegistry,
    name: &str,
    active_skills: &[String],
    stdout: &mut W,
) {
    let skill = match registry.get(name) {
        Some(s) => s,
        None => {
            writeln!(
                stdout,
                "{}Skill '{}' not found.{} Use {}/skills{} to list available skills.",
                RED, name, RESET, BOLD, RESET
            )
            .unwrap_or(());
            return;
        }
    };

    let source_label = match skill.source {
        SkillSource::Personal => "Personal (~/.tinyharness/skills/)",
        SkillSource::Project => "Project (.tinyharness/skills/)",
    };

    writeln!(stdout).unwrap_or(());
    writeln!(
        stdout,
        "{}╭─ Skill: {}{}{} ──────────────────────────╮{}",
        BOLD, CYAN, skill.name, BOLD, RESET
    )
    .unwrap_or(());
    writeln!(
        stdout,
        "{}│{}   {}Description:{} {}",
        BOLD, RESET, BOLD, RESET, skill.description
    )
    .unwrap_or(());
    writeln!(
        stdout,
        "{}│{}   {}Source:{} {}",
        BOLD, RESET, BOLD, RESET, source_label
    )
    .unwrap_or(());
    writeln!(
        stdout,
        "{}│{}   {}Path:{} {}",
        BOLD,
        RESET,
        BOLD,
        RESET,
        skill.path.display()
    )
    .unwrap_or(());

    if let Some(hint) = &skill.argument_hint {
        writeln!(
            stdout,
            "{}│{}   {}Argument hint:{} {}",
            BOLD, RESET, BOLD, RESET, hint
        )
        .unwrap_or(());
    }

    if let Some(compat) = &skill.compatibility {
        writeln!(
            stdout,
            "{}│{}   {}Compatibility:{} {}",
            BOLD, RESET, BOLD, RESET, compat
        )
        .unwrap_or(());
    }

    if let Some(lic) = &skill.license {
        writeln!(
            stdout,
            "{}│{}   {}License:{} {}",
            BOLD, RESET, BOLD, RESET, lic
        )
        .unwrap_or(());
    }

    let auto_label = if skill.disable_model_invocation {
        format!(
            "{}Manual invocation only (model cannot auto-invoke){}",
            ORANGE, RESET
        )
    } else {
        format!("{}Model can auto-invoke this skill{}", GREEN, RESET)
    };
    writeln!(
        stdout,
        "{}│{}   {}Auto-invoke:{} {}",
        BOLD, RESET, BOLD, RESET, auto_label
    )
    .unwrap_or(());

    let active_label = if active_skills
        .iter()
        .any(|s| s.eq_ignore_ascii_case(&skill.name))
    {
        format!("{}● Active{}", GREEN, RESET)
    } else {
        format!("{}○ Inactive{}", GRAY, RESET)
    };
    writeln!(
        stdout,
        "{}│{}   {}Status:{} {}",
        BOLD, RESET, BOLD, RESET, active_label
    )
    .unwrap_or(());

    writeln!(
        stdout,
        "{}╰──────────────────────────────────────────────╯{}",
        BOLD, RESET
    )
    .unwrap_or(());

    // Show the skill content
    if !skill.content.is_empty() {
        writeln!(stdout).unwrap_or(());
        writeln!(stdout, "{}Skill instructions:{}", BOLD, RESET).unwrap_or(());
        writeln!(stdout).unwrap_or(());
        for line in skill.content.lines() {
            writeln!(stdout, "  {}", line).unwrap_or(());
        }
        writeln!(stdout).unwrap_or(());
    }
}

/// Print help for the /skill command.
pub fn execute_help() {
    println!();
    println!("{}Skill management — subcommands:{}", BOLD, RESET);
    println!();
    println!(
        "  {}{:<16}{} List all available skills",
        CYAN, "list", RESET
    );
    println!(
        "  {}{:<16}{} Show details and content of a skill",
        CYAN, "<name>", RESET
    );
    println!();
    println!(
        "{}Tip:{} Use {}/use <name>{} to activate a skill, {}/unload <name>{} to deactivate it.",
        GRAY, RESET, BOLD, RESET, BOLD, RESET
    );
    println!(
        "      Skills are loaded from {}~/.tinyharness/skills/<name>/SKILL.md{} (personal)",
        BOLD, RESET
    );
    println!(
        "      and {}.tinyharness/skills/<name>/SKILL.md{} (project-local).",
        BOLD, RESET
    );
    println!();
}
