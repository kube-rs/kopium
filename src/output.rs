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
    /// Additional field level annotations
    ///
    /// This is currently used by optional builders.
    pub extra_annot: Vec<String>,
    /// Documentation properties extracted from the property
    pub docs: Option<String>,
}

impl Container {
    pub fn uses_btreemaps(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("BTreeMap"))
    }

    pub fn uses_hashmaps(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("HashMap"))
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

    pub fn is_root(&self) -> bool {
        self.level == 0
    }

    pub fn is_main_container(&self) -> bool {
        self.level == 1 && self.name.ends_with("Spec")
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

    /// Add builder annotations
    pub fn builder_fields(&mut self) {
        for m in &mut self.members {
            if m.type_.starts_with("Option<") {
                m.extra_annot
                    .push("#[builder(default, setter(strip_option))]".to_string());
            } else if m.type_.starts_with("Vec<") || m.type_.starts_with("BTreeMap<") {
                m.extra_annot.push("#[builder(default)]".to_string());
            }
        }
    }
}

impl Output {
    /// Rename all structs and all all their members to rust conventions
    ///
    /// Converts [*].members[*].name to snake_case for structs, PascalCase for enums,
    /// and adds a serde(rename = "orig_name") annotation to `serde_annot`.
    ///
    /// It is unsound to skip this step. Some CRDs use kebab-cased members is invalid in Rust.
    pub fn rename(mut self) -> Self {
        for c in &mut self.0 {
            c.rename()
        }
        self
    }

    /// Add builders to all output members
    ///
    /// Adds #[builder(default, setter(strip_option))] to all option types.
    /// Adds #[builder(default)] to required vec and btreemaps.
    pub fn builder_fields(mut self, builders: bool) -> Self {
        if builders {
            for c in &mut self.0 {
                c.builder_fields()
            }
        }
        self
    }
}
