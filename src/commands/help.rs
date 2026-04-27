use super::CommandDispatcher;

pub fn execute() {
    println!("\n{}Available commands:{}", crate::BOLD, crate::RESET);
    for (name, desc) in CommandDispatcher::command_descriptions() {
        println!("  {}{:<20}{} {}", crate::BLUE, name, crate::RESET, desc);
    }
    println!();
}
