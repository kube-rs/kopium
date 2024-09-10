#[macro_use] extern crate log;

mod analyzer;
pub use analyzer::{analyze, Config};
mod output;
pub use output::{format_docstr, Container, MapType, Member, Output};
mod derive;
pub use derive::Derive;
