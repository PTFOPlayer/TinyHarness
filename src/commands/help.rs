use std::io::Write;

use tinyharness_ui::output::Output;

use tinyharness_ui::style::*;

pub fn execute(out: &mut Output, descriptions: &[(&'static str, &'static str)]) {
    let _ = writeln!(out, "\n{BOLD}Available commands:{RESET}");
    for (name, desc) in descriptions {
        let _ = writeln!(out, "  {BLUE}{name:<20}{RESET} {desc}");
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{captured_output, captured_string, strip_ansi};

    #[test]
    fn help_lists_commands() {
        let (mut out, buf) = captured_output();
        let descs = vec![
            ("/help", "Show this help message"),
            ("/mode", "Show or switch mode"),
            ("/exit", "Exit the application"),
        ];
        execute(&mut out, &descs);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Available commands:"));
        assert!(plain.contains("/help"));
        assert!(plain.contains("Show this help message"));
        assert!(plain.contains("/mode"));
        assert!(plain.contains("/exit"));
    }

    #[test]
    fn help_with_empty_descriptions() {
        let (mut out, buf) = captured_output();
        execute(&mut out, &[]);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Available commands:"));
        // Should still print the header even with no commands
        assert!(plain.trim().lines().count() >= 1);
    }
}
