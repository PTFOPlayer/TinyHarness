use crate::style::*;

pub fn execute(descriptions: &[(&'static str, &'static str)]) {
    println!("\n{}Available commands:{}", BOLD, RESET);
    for (name, desc) in descriptions {
        println!("  {}{:<20}{} {}", BLUE, name, RESET, desc);
    }
    println!();
}
