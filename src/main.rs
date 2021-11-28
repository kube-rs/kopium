#[macro_use] extern crate log;
use anyhow::Result;
use clap::{App, Arg};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kopium::{analyze, OutputStruct};
use kube::{Api, Client};
use quote::format_ident;

const KEYWORDS: [&str; 23] = [
    "for", "impl", "continue", "enum", "const", "break", "as", "move", "mut", "mod", "pub", "ref", "self",
    "static", "struct", "super", "true", "trait", "type", "unsafe", "use", "where", "while",
];

#[tokio::main]
async fn main() -> Result<()> {
    let matches = App::new("kopium")
        .version(clap::crate_version!())
        .author("clux <sszynrae@gmail.com>")
        .about("Kubernetes OPenapI UnMangler")
        .arg(
            Arg::new("crd")
                .about("Give the name of the input CRD to use e.g. prometheusrules.monitoring.coreos.com")
                .required(true)
                .index(1),
        )
        .get_matches();
    env_logger::init();

    let client = Client::try_default().await?;
    let api: Api<CustomResourceDefinition> = Api::all(client);
    let crd_name = matches.value_of("crd").unwrap();
    let crd = api.get(crd_name).await?;


    let mut data = None;
    let mut picked_version = None;

    // TODO: pick most suitable version or take arg for it
    let versions = crd.spec.versions;
    if let Some(v) = versions.first() {
        picked_version = Some(v.name.clone());
        if let Some(s) = &v.schema {
            if let Some(schema) = &s.open_api_v3_schema {
                data = Some(schema.clone())
            }
        }
    }
    let kind = crd.spec.names.kind;
    let plural = crd.spec.names.plural;
    let group = crd.spec.group;
    let version = picked_version.expect("need one version in the crd");
    let scope = crd.spec.scope;


    if let Some(schema) = data {
        let mut results = vec![];
        debug!("schema: {}", serde_json::to_string_pretty(&schema)?);
        analyze(schema, "", &kind, 0, &mut results)?;

        print_prelude(&results);
        for s in results {
            if s.level == 0 {
                continue; // ignoring root struct
            } else {
                if s.level == 1 && s.name.ends_with("Spec") {
                    println!("#[derive(CustomResource, Serialize, Deserialize, Clone, Debug)]");
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
                println!("}}")
            }
        }
    } else {
        error!("no schema found for crd {}", crd_name);
    }

    Ok(())
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
