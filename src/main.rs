#[macro_use] extern crate log;
use anyhow::{bail, Result};
use clap::{App, Arg};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, JSONSchemaProps, JSONSchemaPropsOrArray, JSONSchemaPropsOrBool,
};
use kube::{Api, Client};
use quote::format_ident;
use std::collections::HashMap;

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
        analyze(schema, &kind, "", "", 0, &mut results)?;

        print_prelude();
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
                } else {
                    println!("#[derive(Serialize, Deserialize, Clone, Debug)]");
                }
                println!("pub struct {} {{", s.name);
                for m in s.members {
                    if let Some(annot) = m.field_annot {
                        println!("    {}", annot);
                    }
                    let safe_name = if KEYWORDS.contains(&m.name.as_ref()) {
                        format_ident!("r#{}", m.name)
                    } else {
                        format_ident!("{}", m.name)
                    };
                    println!("    pub {}: {},", safe_name, m.type_);
                }
                println!("}}")
            }
        }
    } else {
        error!("no schema found for crd {}", crd_name);
    }

    Ok(())
}

fn print_prelude() {
    println!("use kube::CustomResource;");
    println!("use serde::{{Serialize, Deserialize}};");
    println!("use std::collections::BTreeMap;");
    println!();
}

#[derive(Default, Debug)]
struct OutputStruct {
    // The short name of the struct (kind + capitalized suffix)
    name: String,
    // The full (deduplicated) name of the struct (kind + recursive capitalized suffixes) - unused atm
    dedup_name: String,
    level: u8,
    members: Vec<OutputMember>,
}
#[derive(Default, Debug)]
struct OutputMember {
    name: String,
    type_: String,
    field_annot: Option<String>,
}

const IGNORED_KEYS: [&str; 3] = ["metadata", "apiVersion", "kind"];

/// Scan a schema for structs and members, and recurse to find all structs
///
/// schema: root schema / sub schema
/// kind: crd kind name
/// current: current key name (or empty string for first call)
/// stackname: stacked concat of kind + current_{n-1} + ... + current (used to create dedup_name)
/// level: recursion level (start at 0)
/// results: multable list of generated structs (not deduplicated)
fn analyze(
    schema: JSONSchemaProps,
    kind: &str,
    current: &str,
    stackname: &str,
    level: u8,
    results: &mut Vec<OutputStruct>,
) -> Result<()> {
    let props = schema.properties.unwrap_or_default();
    let mut array_recurse_level: HashMap<String, u8> = Default::default();
    // first generate the object if it is one
    let current_type = schema.type_.unwrap_or_default();
    if current_type == "object" {
        if let Some(JSONSchemaPropsOrBool::Schema(s)) = schema.additional_properties {
            let dict_type = s.type_.unwrap_or_default();
            if !dict_type.is_empty() {
                warn!("not generating type {} - using {} map", current, dict_type);
                return Ok(()); // no members here - it'll be inlined
            }
        }
        let mut members = vec![];
        debug!("Generating struct for {} (under {})", current, stackname);

        let reqs = schema.required.unwrap_or_default();
        // initial analysis of properties (we do not recurse here, we need to find members first)
        for (key, value) in &props {
            let value_type = value.type_.clone().unwrap_or_default();
            let rust_type = match value_type.as_ref() {
                "object" => {
                    let mut dict_key = None;
                    if let Some(additional) = &value.additional_properties {
                        debug!("got additional: {}", serde_json::to_string(&additional)?);
                        if let JSONSchemaPropsOrBool::Schema(s) = additional {
                            let dict_type = s.type_.clone().unwrap_or_default();
                            dict_key = match dict_type.as_ref() {
                                "string" => Some("String".into()),
                                "" => {
                                    if s.x_kubernetes_int_or_string.is_some() {
                                        warn!("coercing presumed IntOrString {} to String", key);
                                        Some("String".into())
                                    } else {
                                        bail!("unknown empty dict type for {}", key)
                                    }
                                }
                                // think the type we get is the value type
                                x => Some(uppercase_first_letter(x)), // best guess
                            };
                        }
                    }
                    if let Some(dict) = dict_key {
                        format!("BTreeMap<String, {}>", dict)
                    } else {
                        let structsuffix = uppercase_first_letter(key);
                        // need to find the deterministic name for the struct
                        format!("{}{}", kind, structsuffix)
                    }
                }
                "string" => "String".to_string(),
                "boolean" => "bool".to_string(),
                "date" => extract_date_type(value)?,
                "number" => extract_number_type(value)?,
                "integer" => extract_integer_type(value)?,
                "array" => {
                    // recurse through repeated arrays until we find a concrete type (keep track of how deep we went)
                    let (array_type, recurse_level) = array_recurse_for_type(value, kind, key, 1)?;
                    debug!(
                        "got array type {} for {} in level {}",
                        array_type, key, recurse_level
                    );
                    array_recurse_level.insert(key.clone(), recurse_level);
                    array_type
                }
                "" => {
                    if value.x_kubernetes_int_or_string.is_some() {
                        warn!("coercing presumed IntOrString {} to String", key);
                        "String".into()
                    } else {
                        bail!("unknown empty dict type for {}", key)
                    }
                }
                x => bail!("unknown type {}", x),
            };

            // Create member and wrap types correctly
            if reqs.contains(key) {
                debug!("with required member {} of type {}", key, rust_type);
                members.push(OutputMember {
                    type_: rust_type,
                    name: key.to_string(),
                    field_annot: None,
                })
            } else {
                // option wrapping possibly needed if not required
                debug!("with optional member {} of type {}", key, rust_type);
                if rust_type.starts_with("BTreeMap") {
                    members.push(OutputMember {
                        type_: rust_type,
                        name: key.to_string(),
                        field_annot: Some(
                            r#"#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]"#.into(),
                        ),
                    })
                } else if rust_type.starts_with("Vec") {
                    members.push(OutputMember {
                        type_: rust_type,
                        name: key.to_string(),
                        field_annot: Some(
                            r#"#[serde(default, skip_serializing_if = "Vec::is_empty")]"#.into(),
                        ),
                    })
                } else {
                    members.push(OutputMember {
                        type_: format!("Option<{}>", rust_type),
                        name: key.to_string(),
                        field_annot: None,
                    })
                }
            }
        }
        // Finalize struct with given members
        results.push(OutputStruct {
            name: format!("{}{}", kind, current),
            dedup_name: format!("{}{}", stackname, current),
            members,
            level,
        });
    }

    // Start recursion for properties
    for (key, value) in props {
        if level == 0 && IGNORED_KEYS.contains(&(key.as_ref())) {
            debug!("not recursing into ignored {}", key); // handled elsewhere
            continue;
        }
        let next_current = uppercase_first_letter(&key);
        let stackname = format!("{}{}", kind, current);
        let value_type = value.type_.clone().unwrap_or_default();
        match value_type.as_ref() {
            "object" => {
                analyze(value, kind, &next_current, &stackname, level + 1, results)?;
            }
            "array" => {
                if let Some(recurse) = array_recurse_level.get(&key).cloned() {
                    let mut inner = value.clone();
                    for _i in 0..recurse {
                        debug!("recursing into props for {}", key);
                        if let Some(sub) = inner.items {
                            match sub {
                                JSONSchemaPropsOrArray::Schema(s) => {
                                    //info!("got inner: {}", serde_json::to_string_pretty(&s)?);
                                    inner = *s.clone();
                                }
                                _ => bail!("only handling single type in arrays"),
                            }
                        } else {
                            bail!("could not recurse into vec");
                        }
                    }
                    analyze(inner, kind, &next_current, &stackname, level + 1, results)?;
                }
            }
            "" => {
                if value.x_kubernetes_int_or_string.is_some() {
                    debug!("not recursing into IntOrString {}", key)
                } else {
                    debug!("not recursing into unknown empty type {}", key)
                }
            }
            x => debug!("not recursing into {} (not a container - {})", key, x),
        }
    }
    Ok(())
}

// recurse into an array type to find its nested type
// this recursion is intialised and ended within a single step of the outer recursion
fn array_recurse_for_type(value: &JSONSchemaProps, kind: &str, key: &str, level: u8) -> Result<(String, u8)> {
    if let Some(items) = &value.items {
        match items {
            JSONSchemaPropsOrArray::Schema(s) => {
                let inner_array_type = s.type_.clone().unwrap_or_default();
                return match inner_array_type.as_ref() {
                    "object" => {
                        let structsuffix = uppercase_first_letter(key);
                        Ok((format!("Vec<{}{}>", kind, structsuffix), level))
                    }
                    "string" => Ok(("Vec<String>".into(), level)),
                    "boolean" => Ok(("Vec<bool>".into(), level)),
                    "date" => Ok((format!("Vec<{}>", extract_date_type(value)?), level)),
                    "number" => Ok((format!("Vec<{}>", extract_number_type(value)?), level)),
                    "integer" => Ok((format!("Vec<{}>", extract_integer_type(value)?), level)),
                    "array" => Ok(array_recurse_for_type(s, kind, key, level + 1)?),
                    x => {
                        bail!("unsupported recursive array type {} for {}", x, key)
                    }
                };
            }
            // maybe fallback to serde_json::Value
            _ => bail!("only support single schema in array {}", key),
        }
    } else {
        bail!("missing items in array type")
    }
}

// ----------------------------------------------------------------------------
// helpers

fn extract_date_type(value: &JSONSchemaProps) -> Result<String> {
    Ok(if let Some(f) = &value.format {
        // NB: these need chrono feature on serde
        match f.as_ref() {
            // Not sure if the first actually works properly..
            // might need a Date<Utc> but chrono docs advocated for NaiveDate
            "date" => "NaiveDate".to_string(),
            "date-time" => "DateTime<Utc>".to_string(),
            x => {
                bail!("unknown date {}", x);
            }
        }
    } else {
        "String".to_string()
    })
}

fn extract_number_type(value: &JSONSchemaProps) -> Result<String> {
    Ok(if let Some(f) = &value.format {
        match f.as_ref() {
            "float" => "f32".to_string(),
            "double" => "f64".to_string(),
            x => {
                bail!("unknown number {}", x);
            }
        }
    } else {
        "f64".to_string()
    })
}

fn extract_integer_type(value: &JSONSchemaProps) -> Result<String> {
    // Think go types just do signed ints, but set a minimum to zero..
    // TODO: look for minimum zero and use to set u32/u64
    Ok(if let Some(f) = &value.format {
        match f.as_ref() {
            "int32" => "i32".to_string(),
            "int64" => "i64".to_string(),
            // TODO: byte / password here?
            x => {
                bail!("unknown integer {}", x);
            }
        }
    } else {
        "i64".to_string()
    })
}

fn uppercase_first_letter(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
