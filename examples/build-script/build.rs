use anyhow::{Context, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::ResourceExt;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let src_dir = initialize_source_dir()?;

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

    let header = format!("// WARNING: automatically generated - manual changes will be overwritten\n\n",);

    let crd_dir = src_dir.join("crds");
    let mut imports = String::new();

    for crd_value in crd_values {
        let crd: CustomResourceDefinition =
            serde_yaml::from_value(crd_value).context("could read crd as CustomResourceDefinition")?;
        let name = crd.name_any(); // kube method on a Resource
        let path = crd_dir.join(&name).with_extension("rs");

        // generate and write
        let generated = generator
            .generate_rust_types_for(&crd, Option::<String>::None)
            .await?;
        if let Err(error) = fs::write(&path, generated).context("failed to write generated file") {
            log::error!("failed to write generated types to: {}", path.display());
            log::error!("{error:#?}\n");
            continue;
        }
        log::info!("wrote generated types in {}", path.display());

        // prepare import statement from crds.rs
        if let Err(_) = writeln!(&mut imports, "pub mod {name};") {
            log::error!("failed to add generated `{name}` module to crds.rs");
        }
    }

    let import_rs = src_dir.join("crds.rs");
    fs::write(&import_rs, header)?;
    fs::write(&import_rs, imports)?;

    log::info!("wrote `crds` module to: {}", import_rs.display());

    Ok(())
}

pub fn multidoc_deserialize(data: &str) -> Result<Vec<serde_yaml::Value>> {
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
        log::info!("wrote crate `crds.rs` file to: {}", gen_rs.display());
    }

    if !gen_folder.is_dir() {
        std::fs::create_dir_all(&gen_folder).context("failed to create crate `src/crds` directory")?;
        log::info!("created crate `src/crds` directory: {}", gen_folder.display());
    }

    Ok(path)
}
