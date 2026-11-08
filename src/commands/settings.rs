use std::io::Write;

use tinyharness_lib::Paths;
use tinyharness_lib::config::{AutoAcceptMode, load_settings_from};
use tinyharness_ui::output::Output;

use crate::commands::registry::CommandResult;
use tinyharness_ui::style::*;

pub fn execute(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    match arg {
        Some(sub) if sub.to_lowercase() == "all" => execute_all(out, &load_settings_from(paths)),
        Some(other) => {
            let _ = writeln!(
                out,
                "{ORANGE}Unknown argument '{other}'.{RESET} Use {BOLD}/settings{RESET} to show settings or {BOLD}/settings all{RESET} to list all safe commands.",
            );
        }
        None => execute_summary(out, &load_settings_from(paths)),
    }
    Ok(CommandResult::Ok)
}

fn execute_summary(out: &mut Output, settings: &tinyharness_lib::config::Settings) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{BOLD}╭─ Settings ─────────────────────────────────╮{RESET}",
    );

    let provider_str = format!("{}", settings.last_provider);
    let _ = writeln!(out, "{BOLD}│{RESET} Provider:  {BLUE}{provider_str}{RESET}",);

    match settings.get_current_model() {
        Some(model) => {
            let _ = writeln!(out, "{BOLD}│{RESET} Model:     {BLUE}{model}{RESET}");
        }
        None => {
            let _ = writeln!(out, "{BOLD}│{RESET} Model:     {ORANGE}none{RESET}");
        }
    }

    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Mode:      {BLUE}{}{RESET}",
        settings.preferred_mode,
    );

    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Timeout:   {BLUE}{}s{RESET}",
        settings.effective_timeout_secs(),
    );
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Retries:   {BLUE}{}{RESET}",
        settings.effective_max_retries(),
    );

    let (st_str, st_color) = if settings.show_thinking {
        ("on", GREEN)
    } else {
        ("off", ORANGE)
    };
    let _ = writeln!(out, "{BOLD}│{RESET} Show Think: {st_color}{st_str}{RESET}",);

    match settings.context_limit {
        Some(limit) => {
            let _ = writeln!(out, "{BOLD}│{RESET} Ctx Limit: {BLUE}{limit} tokens{RESET}",);
        }
        None => {
            let _ = writeln!(
                out,
                "{BOLD}│{RESET} Ctx Limit: {GRAY}auto (model default){RESET}",
            );
        }
    }

    let (auto_accept_str, auto_accept_color) = match settings.auto_accept_mode {
        AutoAcceptMode::All => ("ALL", GREEN),
        AutoAcceptMode::Safe => ("safe commands", GREEN),
        AutoAcceptMode::Off => ("off", ORANGE),
    };
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Auto-Accept: {auto_accept_color}{auto_accept_str}{RESET}",
    );

    let (ac_str, ac_color) = if settings.auto_compact_enabled {
        ("on", GREEN)
    } else {
        ("off", ORANGE)
    };
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Auto-Compact: {ac_color}{ac_str}{RESET}",
    );

    let (q_str, q_color) = if settings.questions_enabled {
        ("on", GREEN)
    } else {
        ("off", ORANGE)
    };
    let _ = writeln!(out, "{BOLD}│{RESET} Questions:  {q_color}{q_str}{RESET}",);

    let safe_commands = settings.get_safe_commands();
    let denied_commands = settings.get_denied_commands();
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Safe Cmds:  {BLUE}{}{RESET} configured (use {BOLD}/settings all{RESET} to list all)",
        safe_commands.len(),
    );
    if !denied_commands.is_empty() {
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} Denied Cmds: {RED}{}{RESET} always denied",
            denied_commands.len(),
        );
    }

    let _ = writeln!(
        out,
        "{BOLD}╰────────────────────────────────────────────╯{RESET}",
    );
    let _ = writeln!(out);
}

fn execute_all(out: &mut Output, settings: &tinyharness_lib::config::Settings) {
    let safe_commands_count = settings.get_safe_commands().len();
    let using_defaults = settings.safe_command_prefixes.is_none();

    let safe_title = if using_defaults {
        format!("All Safe Commands ({safe_commands_count} defaults)")
    } else {
        format!("All Safe Commands ({safe_commands_count} configured)")
    };

    crate::commands::command::render_command_boxes(
        out,
        settings,
        &safe_title,
        None,
        "Always-Deny Commands",
    );
}
