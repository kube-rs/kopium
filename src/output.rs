use std::{cell::OnceCell, sync::OnceLock};

use heck::{ToPascalCase, ToSnakeCase};
use regex::{Regex, RegexBuilder};

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
    pub supports_derive_default: OnceCell<bool>,
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

    pub fn is_status_container(&self) -> bool {
        self.level == 1 && self.name.ends_with("Status")
    }

    pub fn contains_conditions(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("Vec<Condition>"))
    }

    pub fn contains_object_ref(&self) -> bool {
        self.members.iter().any(|m| m.type_.contains("ObjectReference"))
    }

    /// Checks if default is implemented for all props, and if not, returns false
    ///
    /// Behavior for --smart-derive-elision.
    pub fn can_derive_default(&self, containers: &[Container]) -> bool {
        if self.is_enum {
            // Need to drop Default from enum as this cannot be derived.
            // Enum defaults need to either be manually derived
            // or we can insert enum defaults
            return false;
        }

        if let Some(can_derive) = self.supports_derive_default.get() {
            // only run recursive check against every struct once
            return *can_derive;
        }

        for m in &self.members {
            // If the type contains a <, it's a container type. All kopium containers (Map, Vec, Option) has impl Default.
            // If the first character is lowercase, assume it's a built-in type and skip the check.
            if !m.type_.contains('<')
                && m.type_ != "String"
                && m.type_ != "IntOrString"
                && m.type_ != "NaiveDate"
                && m.type_ != "DateTime"
                && m.type_.chars().next().unwrap_or_default().is_uppercase()
                && containers
                    .iter()
                    .find(|c| c.name == m.type_)
                    .is_some_and(|c| !c.can_derive_default(containers))
            {
                self.supports_derive_default.set(false).unwrap();
                return false;
            }
        }

        // No members prevented us from deriving default.
        self.supports_derive_default.set(true).unwrap();
        true
    }
}

impl Container {
    /// Rename all struct members to rust conventions
    pub fn rename(&mut self) {
        let mut seen = vec![]; // track names we output to avoid generating duplicates
        for (i, m) in self.members.iter_mut().enumerate() {
            let mut new_name = if self.is_enum {
                // There are no rust keywords that start uppercase,
                // making this name always a valid identifier except if it contains
                // or starts with an invalid character.
                //
                // `` -> `KopiumEmpty`
                // `mod` -> `Mod`
                // `301` -> `301` -> `r#301` -> `r#_301`
                // `!=` -> `!=` -> `r#!=` -> `r#_!=` -> `KopiumVariant{i}`
                let name = if m.name.is_empty() {
                    "KopiumEmpty".to_owned()
                } else if m.name == "-" {
                    "KopiumDash".to_owned()
                } else if m.name == "_" {
                    "KopiumUnderscore".to_owned()
                } else {
                    m.name.to_pascal_case()
                };

                Container::try_escape_name(name).unwrap_or_else(|| format!("KopiumVariant{i}"))
            } else if m.name == "-" {
                "kopium_dash".to_owned()
            } else if m.name == "_" {
                "kopium_underscore".to_owned()
            } else {
                Container::try_escape_name(m.name.to_snake_case())
                    .unwrap_or_else(|| panic!("invalid field name '{}' could not be escaped", m.name))
            };
            // The new, Rust correct name MIGHT clash with existing names in degenerate cases
            // such as those in https://github.com/kube-rs/kopium/issues/165
            // so if duplicates are seen, we suffix an "X" to disamgiguate (repeatedly if needed)
            while seen.contains(&new_name) {
                let disambiguation_suffix = if self.is_enum { "X" } else { "_x" };
                new_name = format!("{new_name}{disambiguation_suffix}"); // force disambiguate
            }
            seen.push(new_name.clone());

            if new_name != m.name {
                m.serde_annot.push(format!("rename = \"{}\"", m.name));
                m.name = new_name;
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

    /// Tries to escape a field or variant name into a valid Rust identifier.
    fn try_escape_name(name: String) -> Option<String> {
        if syn::parse_str::<syn::Ident>(&name).is_ok() {
            return Some(name);
        }

        let escaped_name = format!("r#{name}");
        if syn::parse_str::<syn::Ident>(&escaped_name).is_ok() {
            return Some(escaped_name);
        }

        let escaped_name = format!("r#_{name}");
        if syn::parse_str::<syn::Ident>(&escaped_name).is_ok() {
            return Some(escaped_name);
        }

        None
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
            c.rename();
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

/// Type used for additionalProperties maps
#[derive(clap::ValueEnum, Clone, Copy, Default, Debug)]
#[clap(rename_all = "PascalCase")]
pub enum MapType {
    #[default]
    BTreeMap,
    HashMap,
}
impl MapType {
    pub fn name(&self) -> &str {
        match self {
            Self::BTreeMap => "BTreeMap",
            Self::HashMap => "HashMap",
        }
    }
}

pub fn format_docstr(indent: &str, input: &str) -> String {
    static RE_CODEBLOCK: OnceLock<Regex> = OnceLock::new();
    let re = RE_CODEBLOCK.get_or_init(|| {
        RegexBuilder::new(r"```.*\n([\s\S]+)\n```")
            .swap_greed(true)
            .build()
            .unwrap()
    });
    let cleaned_input = re.replace_all(input, "```text\n$1\n```");
    // TODO: maybe logic to split doc strings by sentence / length here

    format!(
        "{}/// {}",
        indent,
        cleaned_input.replace('\n', &format!("\n{}/// ", indent))
    )
}

// unit tests
#[cfg(test)]
mod test {
    use super::{format_docstr, Container, Member};
    fn name_only_enum_member(name: &str) -> Member {
        Member {
            name: name.to_string(),
            type_: "".to_string(),
            serde_annot: vec![],
            extra_annot: vec![],
            docs: None,
        }
    }
    fn name_only_int_member(name: &str) -> Member {
        Member {
            name: name.to_string(),
            type_: "u32".to_string(),
            serde_annot: vec![],
            extra_annot: vec![],
            docs: None,
        }
    }

    #[test]
    fn rename_avoids_producing_name_clashes() {
        let mut c = Container {
            name: "EndpointRelabelingsAction".to_string(),
            level: 1,
            members: vec![
                name_only_enum_member("replace"),
                name_only_enum_member("Replace"),
                name_only_enum_member("hashmod"),
                name_only_enum_member("HashMod"),
                // deliberately contrarian examples
                name_only_enum_member("jwks_uri"),
                name_only_enum_member("jwks-uri"),
                name_only_enum_member("jwksUri"),
                name_only_enum_member("JwksUri"),
            ],
            is_enum: true,
            ..Container::default()
        };

        c.rename();
        assert_eq!(&c.members[0].name, "Replace");
        assert_eq!(&c.members[1].name, "ReplaceX");
        assert_eq!(&c.members[2].name, "Hashmod");
        assert_eq!(&c.members[3].name, "HashMod");
        assert_eq!(&c.members[4].name, "JwksUri");
        assert_eq!(&c.members[5].name, "JwksUriX");
        assert_eq!(&c.members[6].name, "JwksUriXX");
        assert_eq!(&c.members[7].name, "JwksUriXXX");
        assert_eq!(c.members.len(), 8);
        // ditto for a struct
        let mut cs = Container {
            name: "FakeStruct".to_string(),
            level: 1,
            members: vec![
                // deliberately contrarian examples
                name_only_int_member("jwks_uri"),
                name_only_int_member("jwks-uri"),
                name_only_int_member("jwksUri"),
                name_only_int_member("JwksUri"),
            ],
            ..Container::default()
        };
        cs.rename();
        assert_eq!(&cs.members[0].name, "jwks_uri");
        assert_eq!(&cs.members[1].name, "jwks_uri_x");
        assert_eq!(&cs.members[2].name, "jwks_uri_x_x");
        assert_eq!(&cs.members[3].name, "jwks_uri_x_x_x");
    }

    #[test]
    fn can_derive_default() {
        let containers = vec![
            Container {
                name: "Simple".to_string(),
                level: 1,
                ..Container::default()
            },
            Container {
                name: "Enum".to_string(),
                level: 1,
                is_enum: true,
                ..Container::default()
            },
            Container {
                name: "Nested".to_string(),
                level: 1,
                members: vec![Member {
                    name: "simple".to_string(),
                    type_: "Simple".to_string(),
                    ..Member::default()
                }],
                ..Container::default()
            },
            Container {
                name: "ReferencesEnum".to_string(),
                level: 1,
                members: vec![Member {
                    name: "enum".to_string(),
                    type_: "Enum".to_string(),
                    ..Member::default()
                }],
                ..Container::default()
            },
            Container {
                name: "ReferencesEnumNested".to_string(),
                level: 1,
                members: vec![Member {
                    name: "references_enum".to_string(),
                    type_: "ReferencesEnum".to_string(),
                    ..Member::default()
                }],
                ..Container::default()
            },
            Container {
                name: "ReferencesEnumOption".to_string(),
                level: 1,
                members: vec![Member {
                    name: "maybe_enum".to_string(),
                    type_: "Option<Enum>".to_string(),
                    ..Member::default()
                }],
                ..Container::default()
            },
            Container {
                name: "ReferencesEnumVec".to_string(),
                level: 1,
                members: vec![Member {
                    name: "enum_list".to_string(),
                    type_: "Vec<Enum>".to_string(),
                    ..Member::default()
                }],
                ..Container::default()
            },
            Container {
                name: "ReferencesEnumNestedOption".to_string(),
                level: 1,
                members: vec![Member {
                    name: "maybe_references_enum".to_string(),
                    type_: "Option<ReferencesEnum>".to_string(),
                    ..Member::default()
                }],
                ..Container::default()
            },
        ];
        assert!(containers[0].can_derive_default(&containers)); // Simple
        assert!(!containers[1].can_derive_default(&containers)); // Enum
        assert!(containers[2].can_derive_default(&containers)); // Nested
        assert!(!containers[3].can_derive_default(&containers)); // ReferencesEnum
        assert!(!containers[4].can_derive_default(&containers)); // ReferencesEnumNested
        assert!(containers[5].can_derive_default(&containers)); // ReferencesEnumOption
        assert!(containers[6].can_derive_default(&containers)); // ReferencesEnumVec
        assert!(containers[7].can_derive_default(&containers)); // ReferencesEnumNestedOption
    }

    #[test]
    fn escapes_codes_from_descriptions() {
        assert_eq!(
            "/// ```text\n/// foobar\n/// ```\n/// ",
            format_docstr("", "```\nfoobar\n```\n")
        );
        assert_eq!(
            "/// Some docs\n/// ```text\n/// foobar\n/// ```\n/// ",
            format_docstr("", "Some docs\n```\nfoobar\n```\n")
        );
        assert_eq!(
            "/// Some docs\n/// ```text\n/// foobar\n/// ```",
            format_docstr("", "Some docs\n```\nfoobar\n```")
        );
        assert_eq!(
            "/// ```text\n/// foobar\n/// ```",
            format_docstr("", "```\nfoobar\n```")
        );
        assert_eq!(
            "/// ```text\n/// foobar('```')\n/// ```",
            format_docstr("", "```\nfoobar('```')\n```")
        );
        assert_eq!(
            "/// ```text\n/// foobar\n/// ```",
            format_docstr("", "```      \nfoobar\n```"),
            "Trailing whitespaces are fine"
        );
        assert_eq!(
            "/// ```text\n/// foobar\n/// ```",
            format_docstr("", "```go\nfoobar\n```"),
            "Language must be removed from code blocks"
        );
        assert_eq!(
            "/// Some docs\n/// with no code blocks!",
            format_docstr("", "Some docs\nwith no code blocks!")
        );
        assert_eq!(
            "/// Some docs\n/// ```text\n/// foobar\n/// ```\n/// Some more docs\n/// ```text\n/// foobar.more\n/// ```",
            format_docstr("", "Some docs\n```\nfoobar\n```\nSome more docs\n```\nfoobar.more\n```")
        );
    }
}
