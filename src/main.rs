use anyhow::{anyhow, Context, Result};
use heck::ToSnakeCase;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceDefinitionVersion, CustomResourceSubresources,
};
use kopium::{analyze, OutputStruct};
use kube::{api, core::Version, Api, Client, ResourceExt};
use quote::format_ident;
use std::path::PathBuf;
use structopt::{clap, StructOpt};

// synced from https://doc.rust-lang.org/reference/keywords.html feb 2022
const KEYWORDS: [&str; 52] = [
    "as",
    "break",
    "const",
    "continue",
    "crate",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "async",
    "await",
    "dyn",
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "macro",
    "override",
    "priv",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    "try",
    "macro_rules",
];

#[derive(StructOpt)]
#[structopt(
    version = clap::crate_version!(),
    author = "clux <sszynrae@gmail.com>",
    about = "Kubernetes OPenapI UnMangler",
)]
struct Kopium {
    /// Give the name of the input CRD to use e.g. prometheusrules.monitoring.coreos.com
    #[structopt(conflicts_with("file"))]
    crd: Option<String>,

    /// Point to the location of a CRD to use on disk
    #[structopt(parse(from_os_str), long = "--filename", short = "f", conflicts_with("crsd"))]
    file: Option<PathBuf>,

    /// Use this CRD version if multiple versions are present
    #[structopt(long)]
    api_version: Option<String>,

    /// Do not emit prelude
    #[structopt(long)]
    hide_prelude: bool,

    /// Do not emit kube derive instructions; structs only
    ///
    /// If this is set, it makes any kube-derive specific options such as `--schema` unnecessary.
    #[structopt(long)]
    hide_kube: bool,

    /// Do not emit inner attributes such as #![allow(non_snake_case)]
    ///
    /// This is useful if you need to consume the code within an include! macro
    /// which does not support inner attributes: https://github.com/rust-lang/rust/issues/47995
    #[structopt(long, short = "i")]
    hide_inner_attr: bool,

    /// Emit doc comments from descriptions
    #[structopt(long, short = "d")]
    docs: bool,

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
    #[structopt(
        long,
        default_value = "disabled",
        possible_values = &["disabled", "manual", "derived"],
    )]
    schema: String,

    /// Derive these extra traits on generated structs
    #[structopt(long,
        short = "D",
        possible_values = &["Copy", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "JsonSchema"],
    )]
    derive: Vec<String>,

    #[structopt(subcommand)]
    command: Option<Command>,

    /// Convert struct members to snake_case
    ///
    /// This will run all members through heck::ToSnakeCase, and if different,
    /// produce a #[serde(rename = "originalName")] attribute on the member.
    ///
    /// This operation is safe because names are preserved through attributes.
    /// However, while not needing the #![allow(non_snake_case)] inner attribute; your code will be longer.
    #[structopt(long, short = "z")]
    snake_case: bool,

    /// Enable all automatation features
    ///
    /// This is a recommended, but early set of features that generates the most rust native code.
    ///
    /// It contains an unstable set of of features and may get expanded in the future.
    ///
    /// Setting --auto enables: --schema=derived --derive=JsonSchema --snake-case --docs
    #[structopt(long, short = "A")]
    auto: bool,
}

#[derive(StructOpt, Clone, Copy, Debug)]
enum Command {
    #[structopt(about = "List available CRDs", setting(clap::AppSettings::Hidden))]
    ListCrds,
    #[structopt(about = "Generate completions", setting(clap::AppSettings::Hidden))]
    Completions {
        #[structopt(about = "The shell to generate completions for", possible_values = &clap::Shell::variants())]
        shell: clap::Shell,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let mut args = Kopium::from_args();
    if args.auto {
        args.docs = true;
        args.snake_case = true;
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
                get_stdin_data().with_context(|| format!("Failed to read from stdin"))?
            } else {
                std::fs::read_to_string(&f).with_context(|| format!("Failed to read {}", f.display()))?
            };

            let crd: CustomResourceDefinition = serde_yaml::from_str(&data)?;
            self.generate(crd).await
        } else {
            match self.command {
                Some(Command::ListCrds) => {
                    let api = Client::try_default()
                        .await
                        .map(Api::<CustomResourceDefinition>::all)?;
                    self.list_crds(api).await
                }
                Some(Command::Completions { shell }) => self.completions(shell),
                None => self.help(),
            }
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

        if let Some(schema) = data {
            let mut structs = vec![];
            log::debug!("schema: {}", serde_json::to_string_pretty(&schema)?);
            analyze(schema, "", &kind, 0, &mut structs)?;

            if !self.hide_prelude {
                self.print_prelude(&structs);
            }

            for s in structs {
                if s.level == 0 {
                    continue; // ignoring root struct
                } else {
                    self.print_docstr(s.docs, "");
                    if s.level == 1 && s.name.ends_with("Spec") {
                        self.print_derives(true);
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
                        println!("pub struct {} {{", s.name);
                    } else {
                        self.print_derives(false);
                        let spec_trimmed_name = s.name.as_str().replace(&format!("{}Spec", kind), &kind);
                        println!("pub struct {} {{", spec_trimmed_name);
                    }
                    for m in s.members {
                        self.print_docstr(m.docs, "    ");
                        let name = if self.snake_case {
                            let converted = m.name.to_snake_case();
                            if converted != m.name {
                                println!("    #[serde(rename = \"{}\")]", m.name);
                            }
                            converted
                        } else {
                            m.name
                        };
                        if let Some(annot) = m.field_annot {
                            println!("    {}", annot);
                        }
                        let safe_name = if KEYWORDS.contains(&name.as_ref()) {
                            format_ident!("r#{}", name)
                        } else {
                            format_ident!("{}", name)
                        };
                        let spec_trimmed_type = m.type_.as_str().replace(&format!("{}Spec", kind), &kind);
                        println!("    pub {}: {},", safe_name, spec_trimmed_type);
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
            println!("{}", crd.name());
        });
        Ok(())
    }

    fn completions(&self, shell: clap::Shell) -> Result<()> {
        let mut completions = Vec::new();
        Self::clap().gen_completions_to("kopium", shell, &mut completions);
        let completions = String::from_utf8(completions)?;
        println!("{}", completions);
        Ok(())
    }

    fn help(&self) -> Result<()> {
        Self::clap().print_help().map(|_| println!())?;
        Ok(())
    }

    fn print_docstr(&self, doc: Option<String>, indent: &str) {
        // print doc strings if requested in arguments
        if self.docs {
            if let Some(d) = doc {
                println!("{}/// {}", indent, d.replace("\n", &format!("\n{}/// ", indent)));
                // TODO: logic to split doc strings by sentence / length here
            }
        }
    }

    fn print_derives(&self, is_root: bool) {
        let mut derives: Vec<String> = vec!["Serialize", "Deserialize", "Clone", "Debug"]
            .into_iter()
            .map(String::from)
            .collect();
        if is_root {
            // CustomResource first for root struct
            derives.insert(0, "CustomResource".to_string());
        }
        derives.extend(self.derive.clone()); // user derives last in user order
        println!("#[derive({})]", derives.join(", "));
    }

    fn print_prelude(&self, results: &[OutputStruct]) {
        if !self.snake_case && !self.hide_inner_attr {
            println!("#![allow(non_snake_case)]");
            println!();
        }
        if !self.hide_kube {
            println!("use kube::CustomResource;");
        }
        if self.derive.contains(&"JsonSchema".to_string()) {
            println!("use schemars::JsonSchema;");
        }
        println!("use serde::{{Serialize, Deserialize}};");
        if results.iter().any(|o| o.uses_btreemaps()) {
            println!("use std::collections::BTreeMap;");
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
                    crd.name(),
                    all_versions(crd)
                )
            })
    } else {
        // pick version with highest version priority
        crd.spec
            .versions
            .iter()
            .max_by_key(|v| Version::parse(&v.name).priority())
            .ok_or_else(|| anyhow!("CRD '{}' has no versions", crd.name()))
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
