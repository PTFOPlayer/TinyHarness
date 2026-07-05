use std::io::Write;

use tinyharness_ui::output::Output;

use tinyharness_ui::style::*;

pub fn execute(out: &mut Output) {
    let _ = writeln!(out, "{ORANGE}Goodbye!{RESET}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{captured_output, captured_string, strip_ansi};

    #[test]
    fn exit_prints_goodbye() {
        let (mut out, buf) = captured_output();
        execute(&mut out);
        let plain = strip_ansi(&captured_string(&buf));
        assert_eq!(plain.trim(), "Goodbye!");
    }
}
