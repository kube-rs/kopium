use anyhow::{Context, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use std::{fmt::Write, fs, path::PathBuf};

macro_rules! p {
    ($($tokens: tt)*) => {
        println!("cargo::warning={}", format!($($tokens)*))
    }
}

fn main() -> Result<()> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=stripped-down-crds.yaml");

    // crd source
    let crds_yaml = std::fs::read_to_string("./stripped-down-crds.yaml").context("could read crd yaml")?;
    let crd_values = multidoc_deserialize(&crds_yaml).context("could deserialize crd yaml as yaml")?;

    // kopium configuration
    let generator = kopium::TypeGenerator::builder()
        .schema_mode(kopium::SchemaMode::Derived)
        .derive(kopium::Derive::all("JsonSchema"))
        .smart_derive_elision(true)
        .emit_docs(true)
        .builders(true)
        .build();

    // TODO: ideally should come from a TypeGenerator method so we can get the kopium version easily
    let header = format!("// WARNING: automatically generated - manual changes will be overwritten\n\n",);

    // prepare output
    let src_dir = initialize_source_dir()?;
    let crd_dir = src_dir.join("crds");
    let mut imports = String::new();
    // p!("found crds {crd_values:?}");

    // generate each crd
    for crd_value in crd_values {
        let crd: CustomResourceDefinition =
            serde_yaml::from_value(crd_value).context("could not read crd as CustomResourceDefinition")?;
        // prom operator has unique kind names
        let name = crd.spec.names.kind.to_lowercase();
        let path = crd_dir.join(&name).with_extension("rs");

        if !["scrapeconfig", "podmonitor", "servicemonitor"].contains(&name.as_ref()) {
            // only doing a couple of the scrape interfaces for the example
            continue;
        }

        // generate and write
        let generated = generator
            .generate_rust_types_for(&crd, Option::<String>::None)
            .context("failed to generate rust types for {name}")?;
        if let Err(error) = fs::write(&path, generated).context("failed to write generated file") {
            p!("failed to write generated types to: {}", path.display());
            p!("{error:#?}\n");
            continue;
        }

        // prepare import statement from crds.rs
        if let Err(_) = writeln!(&mut imports, "pub mod {name};") {
            p!("failed to add generated `{name}` module to crds.rs");
        }
    }

    // generate facade module exporting all crds
    let import_rs = src_dir.join("crds.rs");
    fs::write(&import_rs, header)?;
    fs::write(&import_rs, imports)?;
    Ok(())
}

fn multidoc_deserialize(data: &str) -> Result<Vec<serde_yaml::Value>> {
    use serde::Deserialize;
    let mut docs = vec![];
    for de in serde_yaml::Deserializer::from_str(data) {
        docs.push(serde_yaml::Value::deserialize(de)?);
    }
    Ok(docs)
}

fn initialize_source_dir() -> Result<PathBuf> {
    let path = std::env::current_dir()?;
    let src_dir = path.join("src");

    let gen_folder = src_dir.join("crds");
    let gen_rs = src_dir.join("crds.rs");

    if !gen_rs.is_file() {
        std::fs::write(&gen_rs, "").context("failed to write crate `src/crds.rs`")?;
        p!("wrote crate `crds.rs` file to: {}", gen_rs.display());
    }

    if !gen_folder.is_dir() {
        std::fs::create_dir_all(&gen_folder).context("failed to create crate `src/crds` directory")?;
        p!("created crate `src/crds` directory: {}", gen_folder.display());
    }

    Ok(src_dir)
}
