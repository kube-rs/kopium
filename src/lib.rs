use anyhow::{anyhow, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceDefinitionVersion,
};
use kube::{core::Version, ResourceExt};
use quote::format_ident;
use typed_builder::TypedBuilder;

mod analyzer;

mod derive;
mod output;

pub use self::{
    analyzer::{analyze, Config},
    derive::Derive,
    output::{format_docstr, Container, MapType, Member, Output},
};

#[derive(Clone, Debug, TypedBuilder)]
#[builder(
    field_defaults(default),
    mutators(
        /// Enable all automation features
        ///
        /// This is functionally the same as supplying `--auto` to the `kopium` command
        pub fn auto(&mut self, value: bool) {
            self.emit_docs = value;

            if value {
                self.schema_mode = "derived".into();
            } else {
                self.schema_mode = "disabled".into();
            }
        }

        /// Emit doc comments from CRD field descriptions
        pub fn docs(&mut self, value: bool) {
            self.emit_docs = value;
        }

        /// Set the schema mode to use for `kube-derive`
        ///
        /// This is functionally the same as supplying `--schema` to the `kopium` command
        pub fn schema(&mut self, value: impl Into<String>) {
            self.schema_mode = value.into().to_ascii_lowercase();

            match self.schema_mode.as_str() {
                "disabled" => {
                    self.derive_traits.clear();
                }
                "manual" => {
                    let json = Derive::all("JsonSchema");

                    self.derive_traits.retain(|value| value != &json);
                },
                "derived" => {
                    let json = Derive::all("JsonSchema");

                    if !self.derive_traits.contains(&json) {
                        self.derive_traits.push(json)
                    }
                }
                _ => {}
            }
        }

        /// Add the supplied [`Derive`] directive to the list of traits to be derived on
        /// generated types
        pub fn derive(&mut self, value: Derive) {
            self.derive_traits.push(value);
        }

        /// Attempt to parse the supplied value as a [`Derive`] directive and add it to
        /// the list of traits to be derived on generated types
        pub fn try_derive(&mut self, value: impl AsRef<str>) {
            let derive = value.as_ref().parse::<Derive>().expect("unparsable value");
            self.derive_traits.push(derive);
        }

        /// Add all the supplied [`Derive`] directives to the list of traits to be derived
        /// on generated types
        pub fn derive_all(&mut self, values: impl IntoIterator<Item = Derive>) {
            self.derive_traits.extend(values);
        }

        /// Attempt to parse each of the supplied values as a [`Derive`] directive and add
        /// them to the list of traits to be derived on generated types
        pub fn try_derive_all(&mut self, values: impl IntoIterator<Item = impl AsRef<str>>) {
            let values = values.into_iter().map(|value| {
                value.as_ref().parse::<Derive>().expect("unparsable value")
            });

            self.derive_traits.extend(values);
        }
    )
)]
pub struct KopiumTypeGenerator {
    /// Use this CRD version if multiple versions are present
    api_version: Option<String>,

    /// Do not emit prelude(s)
    hide_prelude: bool,

    /// Do not derive CustomResource nor set kube-derive attributes
    ///
    /// If this is set, it makes any kube-derive specific options such as `--schema` unnecessary
    hide_kube: bool,

    /// Emit doc comments from descriptions
    #[builder(via_mutators)]
    emit_docs: bool,

    /// Emit builder derives via the typed_builder crate
    builders: bool,

    /// Schema mode to use for kube-derive
    ///
    /// The default is --schema=disabled and will compile without a schema,
    /// but the resulting crd cannot be applied into a cluster.
    ///
    /// --schema=manual requires the user to `impl JsonSchema for MyCrdSpec` elsewhere for the code to compile.
    /// Once this is done, the crd via `CustomResourceExt::crd()` can be applied into Kubernetes directly.
    ///
    /// --schema=derived implies `--derive JsonSchema`. The resulting schema will compile without external user action.
    /// The crd via `CustomResourceExt::crd()` can be applied into Kubernetes directly.
    #[builder(
        default_code = r#"String::from("disabled")"#,
        via_mutators(init = String::from("disabled")),
    )]
    schema_mode: String,

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
    #[builder(
        default_code = "Default::default()",
        via_mutators(init = Default::default()),
    )]
    derive_traits: Vec<Derive>,

    /// Elide the following containers from the output
    ///
    /// This allows manual customization of structs from the output without having to remove it from
    /// the output first. Takes precise generated struct names.
    elide: Vec<String>,

    /// Relaxed interpretation
    ///
    /// This allows certain invalid openapi specs to be interpreted as arbitrary objects as used by
    /// argo workflows, for example.
    relaxed: bool,

    /// Disable standardized Condition API
    ///
    /// By default, kopium detects Condition objects and uses a standard
    /// Condition API from k8s_openapi instead of generating a custom definition.
    no_condition: bool,

    /// Disable standardised ObjectReference API
    ///
    /// By default, kopium detects ObjectReference objects and uses a standard
    /// ObjectReference from k8s_openapi instead of generating a custom definition.
    no_object_reference: bool,

    /// Type used to represent maps via additionalProperties
    #[builder(setter(into))]
    map_type: MapType,

    /// Automatically removes `#[derive(Default)]` from structs that contain fields for
    /// which a default cannot be automatically derived.
    ///
    /// This option only has an effect if `--derive Default` is set.
    smart_derive_elision: bool,
}

impl Default for KopiumTypeGenerator {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl KopiumTypeGenerator {
    pub async fn generate_rust_types_for(
        &self,
        crd: &CustomResourceDefinition,
        args: Option<String>,
    ) -> Result<String> {
        use std::fmt::Write;

        let mut generated = String::new();

        let version = find_crd_version(crd, self.api_version.as_deref())?;

        let (kind, plural, group, scope) = (
            &crd.spec.names.kind,
            &crd.spec.names.plural,
            &crd.spec.group,
            &crd.spec.scope,
        );

        self.write_generation_warning(&mut generated, args)?;

        let Some(schema) = version
            .schema
            .as_ref()
            .and_then(|schema| schema.open_api_v3_schema.clone())
        else {
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

        for struct_def in &structs {
            if struct_def.level == 0 {
                continue; // ignoring root struct
            }

            if self.elide.contains(&struct_def.name) {
                log::debug!("eliding {} from the output", struct_def.name);
                continue;
            }

            self.write_docstr(&struct_def.docs, "", &mut generated)?;

            if struct_def.is_main_container() {
                self.write_derives(struct_def, &structs, &mut generated)?;

                //root struct gets kube derives unless opted out
                if !self.hide_kube {
                    writeln!(
                        &mut generated,
                        r#"#[kube(group = "{}", version = "{}", kind = "{}", plural = "{}")]"#,
                        group, &version.name, kind, plural
                    )?;

                    if scope == "Namespaced" {
                        writeln!(&mut generated, r#"#[kube(namespaced)]"#)?;
                    }

                    // status should be listed as a subresource
                    // but also check for top-level .status for certain non-conforming crds like argo application
                    if (version
                        .subresources
                        .as_ref()
                        .is_some_and(|subresource| subresource.status.is_some())
                        || version
                            .schema
                            .as_ref()
                            .and_then(|validation| validation.open_api_v3_schema.as_ref())
                            .and_then(|schema| schema.properties.as_ref())
                            .is_some_and(|mapping| mapping.contains_key("status")))
                        && has_status_resource(&structs)
                    {
                        writeln!(&mut generated, r#"#[kube(status = "{}Status")]"#, kind)?;
                    }

                    if self.schema_mode != "derived" {
                        writeln!(&mut generated, r#"#[kube(schema = "{}")]"#, self.schema_mode)?;
                    }

                    for derive in &self.derive_traits {
                        if derive.derived_trait == "JsonSchema" {
                            continue;
                        }

                        if derive.derived_trait == "Default"
                            && self.smart_derive_elision
                            && !struct_def.can_derive_default(&structs)
                        {
                            continue;
                        }

                        writeln!(&mut generated, r#"#[kube(derive="{}")]"#, derive.derived_trait)?;
                    }
                }

                if struct_def.is_enum {
                    writeln!(&mut generated, "pub enum {} {{", struct_def.name)?;
                } else {
                    writeln!(&mut generated, "pub struct {} {{", struct_def.name)?;
                }
            } else {
                self.write_derives(struct_def, &structs, &mut generated)?;

                let spec_trimmed_name = struct_def.name.as_str().replace(&format!("{}Spec", kind), kind);

                if struct_def.is_enum {
                    writeln!(&mut generated, "pub enum {} {{", spec_trimmed_name)?;
                } else {
                    writeln!(&mut generated, "pub struct {} {{", spec_trimmed_name)?;
                }
            }

            for member in &struct_def.members {
                self.write_docstr(&member.docs, "    ", &mut generated)?;

                if !member.serde_annot.is_empty() {
                    writeln!(&mut generated, "    #[serde({})]", member.serde_annot.join(", "))?;
                }

                let name = format_ident!("{}", member.name);

                for annotation in &member.extra_annot {
                    writeln!(&mut generated, "    {}", annotation)?;
                }

                let spec_trimmed_type = member.type_.as_str().replace(&format!("{}Spec", kind), kind);

                if struct_def.is_enum {
                    // NB: only supporting plain enumerations atm, not oneOf
                    writeln!(&mut generated, "    {},", name)?;
                } else {
                    writeln!(&mut generated, "    pub {}: {},", name, spec_trimmed_type)?;
                }
            }

            writeln!(&mut generated, "}}")?;
            writeln!(&mut generated)?;
        }

        Ok(generated)
    }

    fn write_docstr(
        &self,
        doc: &Option<String>,
        indent: &str,
        buffer: &mut impl std::fmt::Write,
    ) -> Result<()> {
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
    ) -> Result<()> {
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

        writeln!(buffer, "#[derive({})]", derives.join(", ")).map_err(Into::into)
    }

    fn write_prelude(&self, results: &[Container], buffer: &mut impl std::fmt::Write) -> Result<()> {
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

        writeln!(buffer, "}}")?;
        writeln!(buffer, "use self::prelude::*;\n")?;

        Ok(())
    }

    fn write_generation_warning(
        &self,
        buffer: &mut impl std::fmt::Write,
        args: Option<impl std::fmt::Display>,
    ) -> Result<()> {
        writeln!(
            buffer,
            "// WARNING: generated by kopium - manual changes will be overwritten"
        )?;

        if let Some(args) = args {
            writeln!(buffer, "// kopium command: kopium {}", args)?;
        }

        writeln!(buffer, "// kopium version: {}", clap::crate_version!())?;
        writeln!(buffer,)?;

        Ok(())
    }
}

pub fn find_crd_version<'a>(
    crd: &'a CustomResourceDefinition,
    version: Option<&str>,
) -> Result<&'a CustomResourceDefinitionVersion> {
    let mut iter = crd.spec.versions.iter();

    if let Some(version) = version {
        // pick the specified version
        iter.find(|crd_version| crd_version.name == version)
            .ok_or_else(|| {
                anyhow!(
                    "Version '{}' not found in CRD '{}'\navailable versions are '{}'",
                    version,
                    crd.name_any(),
                    all_crd_versions(crd)
                )
            })
    } else {
        // pick the version with the highest priority
        iter.max_by_key(|crd_version| Version::parse(&crd_version.name).priority())
            .ok_or_else(|| anyhow!("CRD '{}' has no versions", crd.name_any()))
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
