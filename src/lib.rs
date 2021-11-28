#[macro_use] extern crate log;

/// Output struct from analysis
#[derive(Default, Debug)]
pub struct OutputStruct {
    // The short name of the struct (kind + capitalized suffix)
    pub name: String,
    pub level: u8,
    pub members: Vec<OutputMember>,
}

/// Output member belonging to an OutputStruct
#[derive(Default, Debug)]
pub struct OutputMember {
    pub name: String,
    pub type_: String,
    pub field_annot: Option<String>,
}

impl OutputStruct {
    pub fn uses_btreemaps(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("BTreeMap"))
    }

    pub fn uses_datetime(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("DateTime"))
    }

    pub fn uses_date(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("NaiveDate"))
    }
}

mod analyzer;
pub use analyzer::analyze;
