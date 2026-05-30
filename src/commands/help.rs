use std::io::Write;

use tinyharness_ui::output::Output;

use crate::style::*;

pub fn execute(out: &mut Output, descriptions: &[(&'static str, &'static str)]) {
    let _ = writeln!(out, "\n{BOLD}Available commands:{RESET}");
    for (name, desc) in descriptions {
        let _ = writeln!(out, "  {BLUE}{name:<20}{RESET} {desc}");
    }
    let _ = writeln!(out);
}
