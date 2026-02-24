#[cfg(feature = "cli")] use std::str::FromStr;

use heck::ToUpperCamelCase;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceDefinitionVersion,
};
use kube::{core::Version, ResourceExt};

mod analyzer;

mod derive;
mod output;

pub use self::{
    analyzer::{analyze, Config},
    derive::Derive,
    output::{format_docstr, Container, MapType, Member, Output},
};

/// Supported values for `kube`'s [`schema`] attribute.
///
/// [schema]: https://docs.rs/kube/latest/kube/derive.CustomResource.html#kubeschema--mode
#[derive(
    // std
    Eq,
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    // strum
    strum::Display,
    strum::IntoStaticStr,
)]
#[cfg_attr(feature = "cli", derive(strum::EnumString))]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
pub enum SchemaMode {
    /// Instruct `kube` to expect `JsonSchema` to be implemented manually for `kopium` generated type(s).
    Manual,

    /// Instruct `kube` to automatically derive a `JsonSchema` implementation for `kopium` generated type(s).
    Derived,

    /// Instruct `kube` to skip deriving a `JsonSchema` implementation for `kopium` generated type(s) entirely.
    ///
    /// **NOTE**: the resulting CRD cannot be applied to a cluster without manual fiddling to add an OpenAPI schema.
    #[default]
    Disabled,
}

#[cfg(feature = "cli")]
impl clap::ValueEnum for SchemaMode {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Manual, Self::Derived, Self::Disabled]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(
            <Self as Into<&'static str>>::into(*self),
        ))
    }
}

#[cfg_attr(feature = "cli", derive(clap::Args))]
#[derive(Clone, Debug, typed_builder::TypedBuilder)]
#[builder(
    field_defaults(default),
    mutators(
        /// Add the supplied [`Derive`] directive to the list
        /// of traits to be derived on generated types
        pub fn derive(&mut self, value: Derive) {
            if !self.derive_traits.contains(&value) {
                self.derive_traits.push(value);
            }
        }

        /// Add all the supplied [`Derive`] directives to the
        /// list of traits to be derived on generated types
        pub fn derive_all(&mut self, values: impl IntoIterator<Item = Derive>) {
            for target in values {
                if !self.derive_traits.contains(&target) {
                    self.derive_traits.push(target);
                }
            }
        }
    )
)]
pub struct TypeGenerator {
    /// Use this CRD version if multiple versions are present
    #[cfg_attr(feature = "cli", arg(long))]
    pub api_version: Option<String>,

    /// Do not emit prelude(s)
    #[cfg_attr(feature = "cli", arg(long))]
    pub hide_prelude: bool,

    /// Do not derive CustomResource nor set kube-derive attributes
    ///
    /// If this is set, it makes any kube-derive specific options such as `--schema` unnecessary
    #[cfg_attr(feature = "cli", arg(long))]
    pub hide_kube: bool,

    /// Emit doc comments from CRD field descriptions
    #[cfg_attr(feature = "cli", arg(short = 'd', long = "docs"))]
    pub emit_docs: bool,

    /// Emit builder derives via the [`typed-builder`](typed_builder) crate
    #[cfg_attr(
        feature = "cli",
        arg(short, long, help = "Emit builder derives via the `typed-builder` crate")
    )]
    pub builders: bool,

    /// Schema mode to use for kube-derive
    ///
    /// The default is `disabled` and will compile without a schema, though the resulting CRD cannot be applied directly to a cluster.
    ///
    /// --schema=manual requires the user to `impl JsonSchema for <generated type>` elsewhere for the code to compile.
    /// Once this is done, the crd via `kube::CustomResourceExt::crd()` can be applied to a cluster directly.
    ///
    /// --schema=derived implies `--derive JsonSchema`. The resulting schema will compile without external user action.
    /// The crd via `CustomResourceExt::crd()` can be applied into Kubernetes directly.
    ///
    /// See: https://docs.rs/kube/latest/kube/derive.CustomResource.html#kubeschema--mode and
    /// https://docs.rs/kube/latest/kube/trait.CustomResourceExt.html#tymethod.crd
    #[builder(default)]
    #[cfg_attr(feature = "cli", arg(long = "schema", default_value_t))]
    pub schema_mode: SchemaMode,

    /// Derive these additional traits on generated objects
    ///
    /// There are three different ways of specifying traits to derive:
    ///
    /// 1. A plain trait name will implement the trait for *all* objects generated from
    ///    the custom resource definition: `--derive PartialEq`
    ///
    /// 2. Constraining the derivation to a singular struct or enum:
    ///    `--derive IssuerAcmeSolversDns01CnameStrategy=PartialEq`
    ///
    /// 3. Constraining the derivation to only structs (@struct), enums (@enum) or *unit-only* enums (@enum:simple),
    ///    meaning enums where no variants are tuple or structs:
    ///    `--derive @struct=PartialEq`, `--derive @enum=PartialEq`, `--derive @enum:simple=PartialEq`
    ///
    /// See also: https://doc.rust-lang.org/reference/items/enumerations.html
    #[cfg_attr(feature = "cli", arg(
        id = "TRAIT",
        short = 'D',
        long = "derive",
        value_parser = Derive::from_str,
        action = clap::ArgAction::Append,
    ))]
    #[builder(via_mutators(init = Default::default()))]
    pub derive_traits: Vec<Derive>,

    /// Elide the following containers from the output
    ///
    /// This allows manual customization of structs from the output without having to remove it from
    /// the output first. Takes precise generated struct names.
    #[cfg_attr(feature = "cli", arg(long, short = 'e'))]
    pub elide: Vec<String>,

    /// Relaxed interpretation
    ///
    /// This allows certain invalid openapi specs to be interpreted as arbitrary objects as used by
    /// argo workflows, for example.
    #[cfg_attr(feature = "cli", arg(long))]
    pub relaxed: bool,

    /// Disable standardized Condition API
    ///
    /// By default, kopium detects Condition objects and uses a standard
    /// Condition API from k8s_openapi instead of generating a custom definition.
    #[cfg_attr(feature = "cli", arg(long))]
    pub no_condition: bool,

    /// Disable standardised ObjectReference API
    ///
    /// By default, kopium detects ObjectReference objects and uses a standard
    /// ObjectReference from k8s_openapi instead of generating a custom definition.
    #[cfg_attr(feature = "cli", arg(long))]
    pub no_object_reference: bool,

    /// Type used to represent maps via `additionalProperties`
    #[builder(setter(into))]
    #[cfg_attr(feature = "cli", arg(long, value_enum, default_value_t))]
    pub map_type: MapType,

    /// Automatically removes `#[derive(Default)]` from structs that contain fields for
    /// which a default cannot be automatically derived.
    ///
    /// This option only has an effect if `--derive Default` is set.
    #[cfg_attr(feature = "cli", arg(long))]
    pub smart_derive_elision: bool,
}

impl Default for TypeGenerator {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl TypeGenerator {
    pub fn generate_rust_types_for(
        &self,
        crd: &CustomResourceDefinition,
        args: Option<impl std::fmt::Display>,
    ) -> anyhow::Result<String> {
        use std::fmt::Write;

        let version = find_crd_version(crd, self.api_version.as_deref())?;

        let data = version
            .schema
            .as_ref()
            .and_then(|schema| schema.open_api_v3_schema.clone());

        let version_name = version.name.clone();

        let (kind, plural, group, scope) = (
            &crd.spec.names.kind,
            &crd.spec.names.plural,
            &crd.spec.group,
            &crd.spec.scope,
        );

        let mut generated = String::new();

        self.write_generation_warning(&mut generated, args)?;

        let Some(schema) = data else {
            anyhow::bail!("no schema found for crd");
        };

        log::debug!("schema: {}", serde_json::to_string_pretty(&schema)?);

        let cfg = Config {
            no_condition: self.no_condition,
            no_object_reference: self.no_object_reference,
            map: self.map_type,
            relaxed: self.relaxed,
        };

        let structs = analyze(schema, kind, cfg)?
            .rename()
            .builder_fields(self.builders)
            .0;

        if !self.hide_prelude {
            self.write_prelude(&structs, &mut generated)?;
        }

        for container in &structs {
            if container.level == 0 {
                continue; // ignoring root struct
            }

            if self.elide.contains(&container.name) {
                log::debug!("eliding {} from the output", container.name);
                continue;
            }

            self.write_docstr(&container.docs, "", &mut generated)?;

            if container.is_main_container() {
                self.write_derives(container, &structs, &mut generated)?;

                //root struct gets kube derives unless opted out
                if !self.hide_kube {
                    writeln!(
                        &mut generated,
                        r#"#[kube(group = "{}", version = "{}", kind = "{}", plural = "{}")]"#,
                        group, version_name, kind, plural
                    )?;

                    if scope == "Namespaced" {
                        writeln!(&mut generated, r#"#[kube(namespaced)]"#)?;
                    }

                    // status should be listed as a subresource
                    // but also check for top-level .status for certain non-conforming crds like argo application
                    if (version.subresources.as_ref().is_some_and(|c| c.status.is_some())
                        || version
                            .schema
                            .as_ref()
                            .and_then(|c| c.open_api_v3_schema.as_ref())
                            .and_then(|c| c.properties.as_ref())
                            .is_some_and(|c| c.contains_key("status")))
                        && has_status_resource(&structs)
                    {
                        writeln!(
                            &mut generated,
                            r#"#[kube(status = "{}Status")]"#,
                            kind.to_upper_camel_case(),
                        )?;
                    }

                    if self.schema_mode != SchemaMode::Derived {
                        writeln!(&mut generated, r#"#[kube(schema = "{}")]"#, self.schema_mode)?;
                    }

                    for derive in &self.derive_traits {
                        if derive.derived_trait == "JsonSchema" {
                            continue;
                        }

                        if derive.derived_trait == "Default"
                            && self.smart_derive_elision
                            && !container.can_derive_default(&structs)
                        {
                            continue;
                        }

                        writeln!(&mut generated, r#"#[kube(derive="{}")]"#, derive.derived_trait)?;
                    }
                }

                if container.is_enum {
                    writeln!(&mut generated, "pub enum {} {{", container.name)?;
                } else {
                    writeln!(&mut generated, "pub struct {} {{", container.name)?;
                }
            } else {
                self.write_derives(container, &structs, &mut generated)?;

                let spec_trimmed_name = container.name.as_str().replace(
                    &format!("{}Spec", kind.to_upper_camel_case()),
                    &kind.to_upper_camel_case(),
                );

                if container.is_enum {
                    writeln!(&mut generated, "pub enum {} {{", spec_trimmed_name)?;
                } else {
                    writeln!(&mut generated, "pub struct {} {{", spec_trimmed_name)?;
                }
            }

            for member in &container.members {
                self.write_docstr(&member.docs, "    ", &mut generated)?;

                if !member.serde_annot.is_empty() {
                    writeln!(&mut generated, "    #[serde({})]", member.serde_annot.join(", "))?;
                }

                let name = quote::format_ident!("{}", member.name);

                for annotation in &member.extra_annot {
                    writeln!(&mut generated, "    {}", annotation)?;
                }

                let spec_trimmed_type = member.type_.as_str().replace(
                    &format!("{}Spec", kind.to_upper_camel_case()),
                    &kind.to_upper_camel_case(),
                );

                if container.is_enum {
                    // NB: only supporting plain enumerations atm, not oneOf
                    writeln!(&mut generated, "    {},", name)?;
                } else {
                    writeln!(&mut generated, "    pub {}: {},", name, spec_trimmed_type)?;
                }
            }

            writeln!(&mut generated, "}}")?;
            writeln!(&mut generated)?;
        }

        let trim_to = generated.trim_end().len();

        generated.truncate(trim_to);
        generated.push('\n');

        Ok(generated)
    }

    fn write_docstr(
        &self,
        doc: &Option<String>,
        indent: &str,
        buffer: &mut impl std::fmt::Write,
    ) -> anyhow::Result<()> {
        // print doc strings if requested in arguments
        if self.emit_docs {
            if let Some(docstring) = doc {
                writeln!(buffer, "{}", format_docstr(indent, docstring))?;
            }
        }

        Ok(())
    }

    fn write_derives(
        &self,
        struct_def: &Container,
        containers: &[Container],
        buffer: &mut impl std::fmt::Write,
    ) -> anyhow::Result<()> {
        let mut derives = vec!["Serialize", "Deserialize", "Clone", "Debug"];

        if struct_def.is_main_container() && !self.hide_kube {
            // CustomResource first for root struct
            derives.insert(0, "CustomResource");
        }

        // TypedBuilder does not work with enums
        if self.builders && !struct_def.is_enum {
            derives.push("TypedBuilder");
        }

        for derive in &self.derive_traits {
            if derive.derived_trait == "Default"
                && ((self.smart_derive_elision && !struct_def.can_derive_default(containers))
                    || struct_def.is_enum)
            {
                continue;
            }

            if derive.is_applicable_to(struct_def) && !derives.contains(&derive.derived_trait.as_str()) {
                derives.push(&derive.derived_trait)
            }
        }

        writeln!(buffer, "#[derive({})]", derives.join(", "))?;

        Ok(())
    }

    fn write_prelude(&self, results: &[Container], buffer: &mut impl std::fmt::Write) -> anyhow::Result<()> {
        writeln!(buffer, "#[allow(unused_imports)]")?;
        writeln!(buffer, "mod prelude {{")?;

        if !self.hide_kube {
            writeln!(buffer, "    pub use kube::CustomResource;")?;
        }

        if self.builders {
            writeln!(buffer, "    pub use typed_builder::TypedBuilder;")?;
        }

        if self
            .derive_traits
            .iter()
            .any(|derive| derive.derived_trait == "JsonSchema")
        {
            writeln!(buffer, "    pub use schemars::JsonSchema;")?;
        }

        writeln!(buffer, "    pub use serde::{{Serialize, Deserialize}};")?;

        if results.iter().any(|container| container.uses_btreemaps()) {
            writeln!(buffer, "    pub use std::collections::BTreeMap;")?;
        }

        if results.iter().any(|container| container.uses_hashmaps()) {
            writeln!(buffer, "    pub use std::collections::HashMap;")?;
        }

        if results.iter().any(|container| container.uses_datetime()) {
            writeln!(buffer, "    pub use chrono::{{DateTime, Utc}};")?;
        }

        if results.iter().any(|container| container.uses_date()) {
            writeln!(buffer, "    pub use chrono::naive::NaiveDate;")?;
        }

        if results.iter().any(|container| container.uses_int_or_string()) {
            writeln!(
                buffer,
                "    pub use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;"
            )?;
        }

        if results.iter().any(|container| container.contains_conditions()) && !self.no_condition {
            writeln!(
                buffer,
                "    pub use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;"
            )?;
        }

        if results.iter().any(|container| container.contains_object_ref()) && !self.no_object_reference {
            writeln!(buffer, "    pub use k8s_openapi::api::core::v1::ObjectReference;")?;
        }

        writeln!(buffer, "}}\n")?;
        writeln!(buffer, "use self::prelude::*;\n")?;

        Ok(())
    }

    fn write_generation_warning(
        &self,
        buffer: &mut impl std::fmt::Write,
        args: Option<impl std::fmt::Display>,
    ) -> anyhow::Result<()> {
        let generated_by = env!("CARGO_PKG_NAME");

        writeln!(
            buffer,
            "// WARNING: generated by {generated_by} - manual changes will be overwritten"
        )?;

        if let Some(args) = args {
            writeln!(buffer, "// {generated_by} command: {generated_by} {}", args)?;
        }

        #[cfg(feature = "cli")]
        let crate_version = clap::crate_version!();

        #[cfg(not(feature = "cli"))]
        let crate_version = env!("CARGO_PKG_VERSION");

        writeln!(buffer, "// {generated_by} version: {crate_version}")?;
        writeln!(buffer,)?;

        Ok(())
    }
}

pub fn find_crd_version<'a>(
    crd: &'a CustomResourceDefinition,
    version: Option<&str>,
) -> anyhow::Result<&'a CustomResourceDefinitionVersion> {
    let mut iter = crd.spec.versions.iter();

    if let Some(version) = version {
        // pick the specified version
        iter.find(|crd_version| crd_version.name == version)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Version '{}' not found in CRD '{}'\navailable versions are '{}'",
                    version,
                    crd.name_any(),
                    all_crd_versions(crd)
                )
            })
    } else {
        // pick the version with the highest priority
        iter.max_by_key(|crd_version| Version::parse(&crd_version.name).priority())
            .ok_or_else(|| anyhow::anyhow!("CRD '{}' has no versions", crd.name_any()))
    }
}

pub fn all_crd_versions(crd: &CustomResourceDefinition) -> String {
    let mut versions = crd
        .spec
        .versions
        .iter()
        .map(|crd_version| crd_version.name.as_str())
        .collect::<Vec<_>>();

    versions.sort_by_cached_key(|version| std::cmp::Reverse(Version::parse(version).priority()));
    versions.join(", ")
}

pub fn has_status_resource(results: &[Container]) -> bool {
    results
        .iter()
        .any(|container| container.is_status_container() && !container.members.is_empty())
}
