use anyhow::{anyhow, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceDefinitionVersion,
};
use kopium::{analyze, OutputStruct};
use kube::{api, core::Version, Api, Client, ResourceExt};
use quote::format_ident;
use structopt::{clap, StructOpt};

const KEYWORDS: [&str; 23] = [
    "for", "impl", "continue", "enum", "const", "break", "as", "move", "mut", "mod", "pub", "ref", "self",
    "static", "struct", "super", "true", "trait", "type", "unsafe", "use", "where", "while",
];

#[derive(StructOpt, Debug)]
#[structopt(
    version = clap::crate_version!(),
    author = "clux <sszynrae@gmail.com>",
    about = "Kubernetes OPenapI UnMangler",
)]
struct Kopium {
    #[structopt(about = "Give the name of the input CRD to use e.g. prometheusrules.monitoring.coreos.com")]
    crd: Option<String>,
    #[structopt(about = "Use this CRD version if multiple versions are present", long)]
    api_version: Option<String>,
    #[structopt(about = "Do not emit prelude", long)]
    hide_prelude: bool,
    #[structopt(about = "Emit doc comments from descriptions", long)]
    docs: bool,
    #[structopt(
        about = "Derive these extra traits on generated structs",
        long,
        possible_values = &["Copy", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash"],
    )]
    derive: Vec<String>,
    #[structopt(subcommand)]
    command: Option<Command>,
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

    Kopium::from_args().dispatch().await
}

impl Kopium {
    async fn dispatch(&self) -> Result<()> {
        let api = Client::try_default()
            .await
            .map(Api::<CustomResourceDefinition>::all)?;
        if let Some(name) = self.crd.as_deref() {
            if self.command.is_none() {
                self.generate(api, name).await
            } else {
                self.help()
            }
        } else {
            match self.command {
                Some(Command::ListCrds) => self.list_crds(api).await,
                Some(Command::Completions { shell }) => self.completions(shell),
                None => self.help(),
            }
        }
    }

    async fn generate(&self, api: Api<CustomResourceDefinition>, name: &str) -> Result<()> {
        let crd = api.get(name).await?;
        let version = self.api_version.as_deref();
        let version = find_crd_version(&crd, version)?;
        let data = version
            .schema
            .as_ref()
            .and_then(|schema| schema.open_api_v3_schema.clone());
        let version = version.name.clone();

        let kind = crd.spec.names.kind;
        let plural = crd.spec.names.plural;
        let group = crd.spec.group;
        let scope = crd.spec.scope;

        if let Some(schema) = data {
            let mut structs = vec![];
            log::debug!("schema: {}", serde_json::to_string_pretty(&schema)?);
            analyze(schema, "", &kind, 0, &mut structs)?;

            if !self.hide_prelude {
                print_prelude(&structs);
            }

            for s in structs {
                if s.level == 0 {
                    continue; // ignoring root struct
                } else {
                    self.print_docstr(s.docs, "");
                    if s.level == 1 && s.name.ends_with("Spec") {
                        self.print_derives();
                        println!(
                            r#"#[kube(group = "{}", version = "{}", kind = "{}", plural = "{}")]"#,
                            group, version, kind, plural
                        );
                        if scope == "Namespaced" {
                            println!(r#"#[kube(namespaced)]"#);
                        }
                        // don't support grabbing original schema atm so disable schemas:
                        // (we coerce IntToString to String anyway so it wont match anyway)
                        println!(r#"#[kube(schema = "disabled")]"#);
                        println!("pub struct {} {{", s.name);
                    } else {
                        println!("#[derive(Serialize, Deserialize, Clone, Debug)]");
                        let spec_trimmed_name = s.name.as_str().replace(&format!("{}Spec", kind), &kind);
                        println!("pub struct {} {{", spec_trimmed_name);
                    }
                    for m in s.members {
                        self.print_docstr(m.docs, "    ");
                        if let Some(annot) = m.field_annot {
                            println!("    {}", annot);
                        }
                        let safe_name = if KEYWORDS.contains(&m.name.as_ref()) {
                            format_ident!("r#{}", m.name)
                        } else {
                            format_ident!("{}", m.name)
                        };
                        let spec_trimmed_type = m.type_.as_str().replace(&format!("{}Spec", kind), &kind);
                        println!("    pub {}: {},", safe_name, spec_trimmed_type);
                    }
                    println!("}}");
                    println!();
                }
            }
        } else {
            log::error!("no schema found for crd {}", name);
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
                println!("{}/// {}", indent, d);
                // TODO: logic to split doc strings by sentence / length here
            }
        }
    }

    fn print_derives(&self) {
        if self.derive.is_empty() {
            println!("#[derive(CustomResource, Serialize, Deserialize, Clone, Debug)]");
        } else {
            println!(
                "#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, {})]",
                self.derive.join(", ")
            );
        }
    }
}


fn print_prelude(results: &[OutputStruct]) {
    println!("use kube::CustomResource;");
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
    println!();
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
