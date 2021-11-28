#[macro_use] extern crate log;

/// Output struct from analysis
#[derive(Default, Debug)]
pub struct OutputStruct {
    // The short name of the struct (kind + capitalized suffix)
    pub name: String,
    // The full (deduplicated) name of the struct (kind + recursive capitalized suffixes) - unused atm
    pub dedup_name: String,
    pub level: u8,
    pub members: Vec<OutputMember>,
}

/// Output member belonging to an OutputStruct
#[derive(Default, Debug)]
pub struct OutputMember {
    pub name: String,
    pub type_: String,
    //pub dedup_type: String,
    pub field_annot: Option<String>,
}

pub mod analyzer;
