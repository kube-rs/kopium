#[macro_use] extern crate log;

mod analyzer;
pub use analyzer::{analyze, Config};
mod output;
pub use output::{Container, MapType, Member, Output};
