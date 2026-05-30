use std::io::Write;

use tinyharness_ui::output::Output;

use crate::style::*;

pub fn execute(out: &mut Output) {
    let _ = writeln!(out, "{ORANGE}Goodbye!{RESET}");
}
