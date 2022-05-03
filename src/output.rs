use heck::{ToPascalCase, ToSnakeCase};

/// All found containers
pub struct Output(pub Vec<Container>);

/// Output container found by analyzer
#[derive(Default, Debug)]
pub struct Container {
    /// The short name of the struct (kind + capitalized suffix)
    pub name: String,
    /// The nestedness level the container was found in
    pub level: u8,
    /// Members or enum members of the container
    pub members: Vec<Member>,
    /// Documentation properties extracted for the container
    pub docs: Option<String>,
    /// Whether this container is an enum
    pub is_enum: bool,
}

/// Output member belonging to an Container
#[derive(Default, Debug)]
pub struct Member {
    /// The raw, unsanitized name of the member
    ///
    /// This must be sanitized against KEYWORDS before it can be printed
    pub name: String,
    /// The stringified name of the type such as BTreeMap<String, EndpointsRelabelings>`
    pub type_: String,
    /// Serde annotations that should prefix the type
    ///
    /// This will be zero or more of:
    /// - default (if the type has a default, or is an option)
    /// - skip_serializing_if = "Option::is_none" (if the type is an Option)
    /// - rename = "orig_name" (if the type does not match rust casing conventions)
    ///
    /// The `rename` attribute is only set if `Container::rename` is called.
    pub serde_annot: Vec<String>,
    pub docs: Option<String>,
}

impl Container {
    pub fn uses_btreemaps(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("BTreeMap"))
    }

    pub fn uses_datetime(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("DateTime"))
    }

    pub fn uses_date(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("NaiveDate"))
    }

    pub fn uses_int_or_string(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("IntOrString"))
    }
}

impl Container {
    /// Rename all struct members to rust conventions
    pub fn rename(&mut self) {
        for m in &mut self.members {
            if self.is_enum {
                let pascald = m.name.to_pascal_case();
                if pascald != m.name {
                    m.serde_annot.push(format!("rename = \"{}\"", m.name));
                }
                m.name = pascald;
            } else {
                // regular container
                let snaked = m.name.to_snake_case();
                if snaked != m.name {
                    m.serde_annot.push(format!("rename = \"{}\"", m.name));
                }
                m.name = snaked;
            }
        }
    }
}

impl Output {
    /// Rename all structs and all all their members to rust conventions
    ///
    /// Converts [*].members[*].name to snake_case for structs, PascalCase for enums,
    /// and adds a serde(rename = "orig_name") annotation to `serde_annot`.
    pub fn rename(mut self) -> Self {
        for c in &mut self.0 {
            c.rename()
        }
        self
    }
}
