use super::CommandDispatcher;
use crate::style::*;

pub fn execute() {
    println!("\n{}Available commands:{}", BOLD, RESET);
    for (name, desc) in CommandDispatcher::command_descriptions() {
        println!("  {}{:<20}{} {}", BLUE, name, RESET, desc);
    }
    println!();
}
