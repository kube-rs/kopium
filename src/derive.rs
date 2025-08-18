use std::str::FromStr;

use anyhow::anyhow;

use crate::Container;

/// Target object for which the trait must be derived.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    /// Derive the trait for all types
    All,
    /// Derive the trait for a named type only.
    Type(String),
    /// Derive the trait for all structs.
    Structs,
    /// Derive the trait for enums, optionally only for simple
    /// ([unit-only](https://doc.rust-lang.org/reference/items/enumerations.html)) enums.
    Enums {
        /// Limit trait derivation to *unit-only* enums.
        unit_only: bool,
    },
}

/// A trait to derive, as well as the object for which to derive it.
#[derive(Debug, Clone, PartialEq)]
pub struct Derive {
    /// Target object (type, structs, enums) to derive the trait for.
    target: Target,
    /// Trait to derive for the target.
    pub derived_trait: String,
}

impl Derive {
    /// Construct a derived trait targeting All objects.
    pub fn all(derived_trait: &str) -> Self {
        Derive {
            target: Target::All,
            derived_trait: derived_trait.to_owned(),
        }
    }

    /// Returns true if this Derive is applicable to the given container.
    ///
    /// See below truth table:
    ///
    /// | Container      \           Target | `All`|`Enum { unit_only: true }`|`Enum { unit_only: false }`|`Struct`|`Type("MyStruct")`|`Type("OtherEnum")`|
    /// |-----------------------------------|------|--------------------------|---------------------------|--------|------------------|-------------------|
    /// |`enum Simple { A, B }`             |`true`|`true`                    |`true`                     |`false` |`false`           |`false`            |
    /// |`enum Complex { A, B { b: bool } }`|`true`|`false`                   |`true`                     |`false` |`false`           |`false`            |
    /// |`struct MyStruct { .. }`           |`true`|`false`                   |`false`                    |`true`  |`true`            |`false`            |
    /// |`enum OtherEnum { A, B }`          |`true`|`false`                   |`false`                    |`true`  |`false`           |`true`             |
    ///
    pub fn is_applicable_to(&self, s: &Container) -> bool {
        match &self.target {
            Target::All => true,
            Target::Type(name) => &s.name == name,
            Target::Structs => !s.is_enum,
            Target::Enums { unit_only } => {
                if !s.is_enum {
                    return false;
                }

                if *unit_only && s.members.iter().any(|member| !member.type_.is_empty()) {
                    return false;
                }

                true
            }
        }
    }
}

impl FromStr for Derive {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some((target, derived_trait)) = value.split_once('=') {
            if target.is_empty() {
                return Err(anyhow!("derive target cannot be empty in '{value}'"));
            };

            if derived_trait.is_empty() {
                return Err(anyhow!("derived trait cannot be empty in '{value}'"));
            }

            let target = if let Some(target) = target.strip_prefix('@') {
                match target {
                    "struct" | "structs" => Target::Structs,
                    "enum" | "enums" => Target::Enums { unit_only: false },
                    "enum:simple" | "enums:simple" => Target::Enums { unit_only: true },
                    other => {
                        return Err(anyhow!(
                            "unknown derive target @{other}, must be one of @struct, @enum, or @enum:simple"
                        ))
                    }
                }
            } else {
                Target::Type(target.to_owned())
            };

            Ok(Derive {
                target,
                derived_trait: derived_trait.to_owned(),
            })
        } else {
            Ok(Derive {
                target: Target::All,
                derived_trait: value.to_owned(),
            })
        }
    }
}

#[cfg(test)]
#[test]
fn derive_applicability() {
    use crate::Member;

    let structure = Container {
        is_enum: false,
        ..Default::default()
    };

    let simple_enum = Container {
        is_enum: true,
        members: vec![Member {
            type_: String::new(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let complex_enum = Container {
        is_enum: true,
        members: vec![Member {
            type_: "SomeNonEmptyType".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let named_structure = Container {
        name: "MyStruct".to_string(),
        is_enum: false,
        ..Default::default()
    };

    let named_enum = Container {
        name: "OtherEnum".to_string(),
        is_enum: true,
        ..Default::default()
    };

    let all_trait = Derive::all("PartialEq");
    assert!(all_trait.is_applicable_to(&structure));
    assert!(all_trait.is_applicable_to(&simple_enum));
    assert!(all_trait.is_applicable_to(&complex_enum));
    assert!(all_trait.is_applicable_to(&named_structure));
    assert!(all_trait.is_applicable_to(&named_enum));

    let simple_enum_trait = Derive {
        target: Target::Enums { unit_only: true },
        derived_trait: "PartialEq".to_string(),
    };
    assert!(simple_enum_trait.is_applicable_to(&simple_enum));
    assert!(!simple_enum_trait.is_applicable_to(&complex_enum));
    assert!(!simple_enum_trait.is_applicable_to(&structure));
    assert!(!simple_enum_trait.is_applicable_to(&named_structure));
    assert!(simple_enum_trait.is_applicable_to(&named_enum));

    let complex_enum_trait = Derive {
        target: Target::Enums { unit_only: false },
        derived_trait: "PartialEq".to_string(),
    };
    assert!(complex_enum_trait.is_applicable_to(&simple_enum));
    assert!(complex_enum_trait.is_applicable_to(&complex_enum));
    assert!(!complex_enum_trait.is_applicable_to(&structure));
    assert!(!complex_enum_trait.is_applicable_to(&named_structure));
    assert!(complex_enum_trait.is_applicable_to(&named_enum));

    let struct_trait = Derive {
        target: Target::Structs,
        derived_trait: "PartialEq".to_string(),
    };
    assert!(!struct_trait.is_applicable_to(&simple_enum));
    assert!(!struct_trait.is_applicable_to(&complex_enum));
    assert!(struct_trait.is_applicable_to(&structure));
    assert!(struct_trait.is_applicable_to(&named_structure));
    assert!(!struct_trait.is_applicable_to(&named_enum));

    let named_struct_trait = Derive {
        target: Target::Type("MyStruct".to_string()),
        derived_trait: "PartialEq".to_string(),
    };
    assert!(!named_struct_trait.is_applicable_to(&simple_enum));
    assert!(!named_struct_trait.is_applicable_to(&complex_enum));
    assert!(!named_struct_trait.is_applicable_to(&structure));
    assert!(named_struct_trait.is_applicable_to(&named_structure));
    assert!(!named_struct_trait.is_applicable_to(&named_enum));
}

#[cfg(test)]
#[test]
fn test_derive_parsing() {
    assert_eq!("PartialEq".parse::<Derive>().unwrap(), Derive::all("PartialEq"));

    assert_eq!("@struct=PartialEq".parse::<Derive>().unwrap(), Derive {
        target: Target::Structs,
        derived_trait: "PartialEq".to_string()
    });

    assert_eq!("@enum=PartialEq".parse::<Derive>().unwrap(), Derive {
        target: Target::Enums { unit_only: false },
        derived_trait: "PartialEq".to_string()
    });

    assert_eq!("@enum:simple=PartialEq".parse::<Derive>().unwrap(), Derive {
        target: Target::Enums { unit_only: true },
        derived_trait: "PartialEq".to_string()
    });

    assert_eq!("MyStruct=PartialEq".parse::<Derive>().unwrap(), Derive {
        target: Target::Type("MyStruct".to_string()),
        derived_trait: "PartialEq".to_string()
    });

    assert_eq!(
        "=".parse::<Derive>().unwrap_err().to_string(),
        "derive target cannot be empty in '='"
    );

    assert_eq!(
        "=PartialEq".parse::<Derive>().unwrap_err().to_string(),
        "derive target cannot be empty in '=PartialEq'"
    );

    assert_eq!(
        "@struct=".parse::<Derive>().unwrap_err().to_string(),
        "derived trait cannot be empty in '@struct='"
    );
}
