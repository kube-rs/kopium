use std::path::PathBuf;
#[macro_use]
extern crate log;
use anyhow::{anyhow, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceDefinitionVersion, CustomResourceSubresources,
};
use kopium::{Analyzer, Container};
use kube::{api, core::Version, Api, Client, ResourceExt};
use quote::format_ident;

#[derive(Parser)]
#[command(
    version = clap::crate_version!(),
    author = "clux <sszynrae@gmail.com>",
    about = "Kubernetes OPenapI UnMangler",
)]
struct Kopium {
    /// Give the name of the input CRD to use e.g. prometheusrules.monitoring.coreos.com
    #[arg(conflicts_with("file"))]
    crd: Option<String>,

    /// Point to the location of a CRD to use on disk
    #[arg(long = "filename", short, conflicts_with("crd"))]
    file: Option<PathBuf>,

    /// Use this CRD version if multiple versions are present
    #[arg(long)]
    api_version: Option<String>,

    /// Do not emit prelude
    #[arg(long)]
    hide_prelude: bool,

    /// Do not emit kube derive instructions; structs only
    ///
    /// If this is set, it makes any kube-derive specific options such as `--schema` unnecessary.
    #[arg(long)]
    hide_kube: bool,

    /// Emit doc comments from descriptions
    #[arg(long, short)]
    docs: bool,

    /// Emit builder derives via the typed_builder crate
    #[arg(long, short)]
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
    #[arg(
        long,
        default_value = "disabled",
        value_parser = ["disabled", "manual", "derived"],
    )]
    schema: String,

    /// Derive these extra traits on generated structs
    #[arg(long,
        short = 'D',
        value_parser = ["Copy", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "JsonSchema"],
    )]
    derive: Vec<String>,

    #[command(subcommand)]
    command: Option<Command>,

    /// Enable all automatation features
    ///
    /// This is a recommended, but early set of features that generates the most rust native code.
    ///
    /// It contains an unstable set of of features and may get expanded in the future.
    ///
    /// Setting --auto enables: --schema=derived --derive=JsonSchema --docs
    #[arg(long, short = 'A')]
    auto: bool,

    /// Elide the following containers from the output
    ///
    /// This allows manual customization of structs from the output without having to remove it from
    /// the output first.
    #[arg(long, short = 'e')]
    elide: Vec<String>,

    /// Disable generation of custom Condition APIs.
    ///
    /// This detects if a particular path is an array of Condition objects and uses a standard
    /// Condition API instead of generating a custom definition.
    #[arg(long)]
    disable_condition_gen: bool,
}

#[derive(Clone, Copy, Debug, Subcommand)]
#[command(args_conflicts_with_subcommands = true)]
enum Command {
    #[command(about = "List available CRDs", hide = true)]
    ListCrds,
    #[command(about = "Generate completions", hide = true)]
    Completions {
        #[arg(help = "The shell to generate completions for")]
        shell: clap_complete::Shell,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    // Ignore SIGPIPE errors to avoid having to use let _ = write! everywhere
    // See https://github.com/rust-lang/rust/issues/46016
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let mut args = Kopium::parse();
    if args.auto {
        args.docs = true;
        args.schema = "derived".into();
    }
    if args.schema == "derived" && !args.derive.contains(&"JsonSchema".to_string()) {
        args.derive.push("JsonSchema".to_string());
    }
    args.dispatch().await
}

fn get_stdin_data() -> Result<String> {
    use std::io::{stdin, Read};
    let mut buf = Vec::new();
    stdin().read_to_end(&mut buf)?;
    let input = String::from_utf8(buf)?;
    Ok(input)
}

impl Kopium {
    async fn dispatch(&self) -> Result<()> {
        if let Some(name) = self.crd.as_deref() {
            let api = Client::try_default()
                .await
                .map(Api::<CustomResourceDefinition>::all)?;
            let crd = api.get(name).await?;
            self.generate(crd).await
        } else if let Some(f) = self.file.as_deref() {
            // no cluster access needed in this case
            let data = if f.to_string_lossy() == "-" {
                get_stdin_data().with_context(|| "Failed to read from stdin".to_string())?
            } else {
                std::fs::read_to_string(f).with_context(|| format!("Failed to read {}", f.display()))?
            };

            let crd: CustomResourceDefinition = serde_yaml::from_str(&data)?;
            self.generate(crd).await
        } else if let Some(command) = self.command {
            match command {
                Command::ListCrds => {
                    let api = Client::try_default()
                        .await
                        .map(Api::<CustomResourceDefinition>::all)?;
                    self.list_crds(api).await
                }
                Command::Completions { shell } => self.completions(shell),
            }
        } else {
            self.help()
        }
    }

    async fn generate(&self, crd: CustomResourceDefinition) -> Result<()> {
        let version = self.api_version.as_deref();
        let version = find_crd_version(&crd, version)?;
        let data = version
            .schema
            .as_ref()
            .and_then(|schema| schema.open_api_v3_schema.clone());
        let version_name = version.name.clone();

        let kind = &crd.spec.names.kind;
        let plural = &crd.spec.names.plural;
        let group = &crd.spec.group;
        let scope = &crd.spec.scope;

        self.print_generation_warning();

        if let Some(schema) = data {
            log::debug!("schema: {}", serde_json::to_string_pretty(&schema)?);
            let analyzer = Analyzer::new(self.disable_condition_gen);
            let structs = analyzer
                .analyze(schema, kind)?
                .rename()
                .builder_fields(self.builders)
                .0;

            if !self.hide_prelude {
                self.print_prelude(&structs);
            }

            for s in &structs {
                if s.level == 0 {
                    continue; // ignoring root struct
                } else {
                    if self.elide.contains(&s.name) {
                        debug!("eliding {} from the output", s.name);
                        continue;
                    }
                    self.print_docstr(&s.docs, "");
                    if s.is_main_container() {
                        self.print_derives(s);
                        //root struct gets kube derives unless opted out
                        if !self.hide_kube {
                            println!(
                                r#"#[kube(group = "{}", version = "{}", kind = "{}", plural = "{}")]"#,
                                group, version_name, kind, plural
                            );
                            if scope == "Namespaced" {
                                println!(r#"#[kube(namespaced)]"#);
                            }
                            if let Some(CustomResourceSubresources { status: Some(_), .. }) =
                                version.subresources
                            {
                                println!(r#"#[kube(status = "{}Status")]"#, kind);
                            }
                            if self.schema != "derived" {
                                println!(r#"#[kube(schema = "{}")]"#, self.schema);
                            }
                        }
                        if s.is_enum {
                            println!("pub enum {} {{", s.name);
                        } else {
                            println!("pub struct {} {{", s.name);
                        }
                    } else {
                        self.print_derives(s);
                        let spec_trimmed_name = s.name.as_str().replace(&format!("{}Spec", kind), kind);
                        if s.is_enum {
                            println!("pub enum {} {{", spec_trimmed_name);
                        } else {
                            println!("pub struct {} {{", spec_trimmed_name);
                        }
                    }
                    for m in &s.members {
                        self.print_docstr(&m.docs, "    ");
                        if !m.serde_annot.is_empty() {
                            println!("    #[serde({})]", m.serde_annot.join(", "));
                        }
                        let name = format_ident!("{}", m.name);
                        for annot in &m.extra_annot {
                            println!("    {}", annot);
                        }
                        let spec_trimmed_type = m.type_.as_str().replace(&format!("{}Spec", kind), kind);
                        if s.is_enum {
                            // NB: only supporting plain enumerations atm, not oneOf
                            println!("    {},", name);
                        } else {
                            println!("    pub {}: {},", name, spec_trimmed_type);
                        }
                    }
                    println!("}}");
                    println!();
                }
            }
        } else {
            log::error!("no schema found for crd");
        }

        Ok(())
    }

    async fn list_crds(&self, api: Api<CustomResourceDefinition>) -> Result<()> {
        let lp = api::ListParams::default();
        api.list(&lp).await?.items.iter().for_each(|crd| {
            println!("{}", crd.name_any());
        });
        Ok(())
    }

    fn completions(&self, shell: clap_complete::Shell) -> Result<()> {
        let mut command = Self::command();
        clap_complete::generate(shell, &mut command, "kopium", &mut std::io::stdout());
        Ok(())
    }

    fn help(&self) -> Result<()> {
        Self::command().print_help()?;
        Ok(())
    }

    fn print_docstr(&self, doc: &Option<String>, indent: &str) {
        // print doc strings if requested in arguments
        if self.docs {
            if let Some(d) = doc {
                println!("{}/// {}", indent, d.replace('\n', &format!("\n{}/// ", indent)));
                // TODO: logic to split doc strings by sentence / length here
            }
        }
    }

    fn print_derives(&self, s: &Container) {
        let mut derives: Vec<String> = vec!["Serialize", "Deserialize", "Clone", "Debug"]
            .into_iter()
            .map(String::from)
            .collect();
        if s.is_main_container() && !self.hide_kube {
            // CustomResource first for root struct
            derives.insert(0, "CustomResource".to_string());
        }
        if self.builders {
            derives.push("TypedBuilder".to_string());
        }
        // add user derives last in order
        for d in &self.derive {
            if s.is_enum && d == "Default" {
                // Need to drop Default from enum as this cannot be derived.
                // Enum defaults need to either be manually derived
                // or we can insert enum defaults
                continue;
            }
            derives.push(d.clone());
        }
        println!("#[derive({})]", derives.join(", "));
    }

    fn print_prelude(&self, results: &[Container]) {
        if !self.hide_kube {
            println!("use kube::CustomResource;");
        }
        if self.builders {
            println!("use typed_builder::TypedBuilder;");
        }
        if self.derive.contains(&"JsonSchema".to_string()) {
            println!("use schemars::JsonSchema;");
        }
        println!("use serde::{{Serialize, Deserialize}};");
        if results.iter().any(|o| o.uses_btreemaps()) {
            println!("use std::collections::BTreeMap;");
        }
        if results.iter().any(|o| o.uses_hashmaps()) {
            println!("use std::collections::HashMap;");
        }
        if results.iter().any(|o| o.uses_datetime()) {
            println!("use chrono::{{DateTime, Utc}};");
        }
        if results.iter().any(|o| o.uses_date()) {
            println!("use chrono::naive::NaiveDate;");
        }
        if results.iter().any(|o| o.uses_int_or_string()) {
            println!("use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;");
        }
        println!();
    }

    fn print_generation_warning(&self) {
        println!("// WARNING: generated by kopium - manual changes will be overwritten");
        let args = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        println!("// kopium command: kopium {}", args);
        println!("// kopium version: {}", clap::crate_version!());
        println!();
    }
}

fn find_crd_version<'a>(
    crd: &'a CustomResourceDefinition,
    version: Option<&str>,
) -> Result<&'a CustomResourceDefinitionVersion> {
    if let Some(version) = version {
        // pick specified version
        crd.spec
            .versions
            .iter()
            .find(|v| v.name == version)
            .ok_or_else(|| {
                anyhow!(
                    "Version '{}' not found in CRD '{}'\navailable versions are '{}'",
                    version,
                    crd.name_any(),
                    all_versions(crd)
                )
            })
    } else {
        // pick version with highest version priority
        crd.spec
            .versions
            .iter()
            .max_by_key(|v| Version::parse(&v.name).priority())
            .ok_or_else(|| anyhow!("CRD '{}' has no versions", crd.name_any()))
    }
}

fn all_versions(crd: &CustomResourceDefinition) -> String {
    let mut vers = crd
        .spec
        .versions
        .iter()
        .map(|v| v.name.as_str())
        .collect::<Vec<_>>();
    vers.sort_by_cached_key(|v| std::cmp::Reverse(Version::parse(v).priority()));
    vers.join(", ")
}
