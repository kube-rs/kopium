use anyhow::anyhow;
use std::str::FromStr;

use crate::Container;

/// Target object for which the trait must be derived.
#[derive(Debug, Clone, PartialEq)]
pub enum DeriveTarget {
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
    pub target: DeriveTarget,
    /// Trait to derive for the target.
    pub derived_trait: String,
}

impl Derive {
    /// Construct a derived trait targeting All objects.
    pub fn all(derived_trait: &str) -> Self {
        Derive {
            target: DeriveTarget::All,
            derived_trait: derived_trait.to_owned(),
        }
    }

    pub fn is_applicable_to(&self, s: &Container) -> bool {
        if s.is_enum && self.derived_trait == "Default" {
            // Need to drop Default from enum as this cannot be derived.
            // Enum defaults need to either be manually derived
            // or we can insert enum defaults
            return false;
        }

        // Only insert the trait if the target matches our container.
        match &self.target {
            DeriveTarget::All => true,
            DeriveTarget::Type(name) => &s.name == name,
            DeriveTarget::Structs => !s.is_enum,
            DeriveTarget::Enums { unit_only } => {
                if s.is_enum {
                    return false;
                }

                if *unit_only && s.members.iter().any(|member| member.type_.is_empty()) {
                    return false;
                }

                true
            }
        }
    }
}

impl FromStr for Derive {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> std::prelude::v1::Result<Self, Self::Err> {
        if let Some((target, derived_trait)) = value.split_once('=') {
            if target.is_empty() {
                return Err(anyhow!("derive target cannot be empty in '{value}'"));
            };

            if derived_trait.is_empty() {
                return Err(anyhow!("derived trait cannot be empty in '{value}'"));
            }

            let target = if let Some(target) = target.strip_prefix('@') {
                match target {
                    "struct" | "structs" => DeriveTarget::Structs,
                    "enum" | "enums" => DeriveTarget::Enums { unit_only: false },
                    "enum:simple" | "enums:simple" => DeriveTarget::Enums { unit_only: true },
                    other => {
                        return Err(anyhow!(
                            "unknown derive target @{other}, must be one of @struct, @enum, or @enum:simple"
                        ))
                    }
                }
            } else {
                DeriveTarget::Type(target.to_owned())
            };

            Ok(Derive {
                target,
                derived_trait: derived_trait.to_owned(),
            })
        } else {
            Ok(Derive {
                target: DeriveTarget::All,
                derived_trait: value.to_owned(),
            })
        }
    }
}
