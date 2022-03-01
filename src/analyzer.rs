//! Deals entirely with schema analysis for the purpose of creating output structs + members
use crate::{OutputMember, OutputStruct};
use anyhow::{bail, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    JSONSchemaProps, JSONSchemaPropsOrArray, JSONSchemaPropsOrBool,
};
use std::collections::{BTreeMap, HashMap};

const IGNORED_KEYS: [&str; 3] = ["metadata", "apiVersion", "kind"];

/// Scan a schema for structs and members, and recurse to find all structs
///
/// schema: root schema / sub schema
/// current: current key name (or empty string for first call) - must capitalize first letter
/// stack: stacked concat of kind + current_{n-1} + ... + current (used to create dedup names/types)
/// level: recursion level (start at 0)
/// results: multable list of generated structs (not deduplicated)
pub fn analyze(
    schema: JSONSchemaProps,
    current: &str,
    stack: &str,
    level: u8,
    results: &mut Vec<OutputStruct>,
) -> Result<()> {
    let props = schema.properties.clone().unwrap_or_default();
    let mut array_recurse_level: HashMap<String, u8> = Default::default();
    // first generate the object if it is one
    let current_type = schema.type_.clone().unwrap_or_default();
    if current_type == "object" {
        // TODO: figure out if we can have both additionalProperties and properties
        if let Some(JSONSchemaPropsOrBool::Schema(s)) = schema.additional_properties.as_ref() {
            let dict_type = s.type_.clone().unwrap_or_default();
            // It's possible to specify the properties inside a nested additionalProperties.properties
            if let Some(extra_props) = &s.properties {
                // in this case we need to run analysis on these nested types
                debug!("Generating nested struct for {} (under {})", current, stack);
                let new_result = analyze_object_properties(
                    &extra_props,
                    stack,
                    &mut array_recurse_level,
                    level,
                    &schema,
                )?;
                results.extend(new_result);
            }
            else if !dict_type.is_empty() {
                warn!("not generating type {} - using {} map", current, dict_type);
                return Ok(()); // no members here - it'll be inlined
            }
        }
        else { // else, regular properties only
            debug!("Generating struct for {} (under {})", current, stack);
            // initial analysis of properties (we do not recurse here, we need to find members first)
            let new_result = analyze_object_properties(
                &props,
                stack,
                &mut array_recurse_level,
                level,
                &schema,
            )?;
            results.extend(new_result);
        }
    }

    // Start recursion for properties
    for (key, value) in props {
        if level == 0 && IGNORED_KEYS.contains(&(key.as_ref())) {
            debug!("not recursing into ignored {}", key); // handled elsewhere
            continue;
        }
        let next_key = uppercase_first_letter(&key);
        let next_stack = format!("{}{}", stack, next_key);
        let value_type = value.type_.clone().unwrap_or_default();
        match value_type.as_ref() {
            "object" => {
                // catch unconventional & ad-hoc definitions of "array" maps within an object's additional props:
                let mut handled_inner = false;
                if let Some(JSONSchemaPropsOrBool::Schema(s)) = &value.additional_properties {
                    let dict_type = s.type_.clone().unwrap_or_default();
                    if dict_type == "array" {
                        // unpack the inner object from the array wrap
                        if let Some(JSONSchemaPropsOrArray::Schema(items)) = &s.as_ref().items {
                            analyze(*items.clone(), &next_key, &next_stack, level + 1, results)?;
                            handled_inner = true;
                        }
                    }
                    // TODO: not sure if these nested recurses are necessary - cluster test case does not have enough data
                    //if let Some(extra_props) = &s.properties {
                    //    for (_key, value) in extra_props {
                    //        debug!("nested recurse into {} {} - key: {}", next_key, next_stack, _key);
                    //        analyze(value.clone(), &next_key, &next_stack, level +1, results)?;
                    //    }
                    //}
                }
                if !handled_inner {
                    // normal object recurse
                    analyze(value, &next_key, &next_stack, level + 1, results)?;
                }
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
                    analyze(inner, &next_key, &next_stack, level + 1, results)?;
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

// helper to figure out what output structs (returned) and embedded members are contained in the current object schema
fn analyze_object_properties(
    props: &BTreeMap<String, JSONSchemaProps>,
    stack: &str,
    array_recurse_level: &mut HashMap<String, u8>,
    level: u8,
    schema: &JSONSchemaProps,
) -> Result<Vec<OutputStruct>, anyhow::Error> {
    let mut results = vec![];
    let mut members = vec![];
    let reqs = schema.required.clone().unwrap_or_default();
    for (key, value) in props {
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
                            "array" => {
                                // possible to inline a struct here for a map (even though it says array)
                                // (openshift agent crd test for struct 'validationsInfo' does this)
                                // for now assume this is a convenience for inline map structs (as actual "array" case is below)
                                // if this is not true; we may need to restrict this case to:
                                // - s.as_ref().items is a Some(JSONSchemaPropsOrArray::Schema(_))
                                // it's also possible that this will need better recurse handling for bigger cases
                                Some(format!("{}{}", stack, uppercase_first_letter(key)))
                            },
                            "object" => {
                                // we can have objects inlined inside additional properties
                                // we THINK this is the Map : String -> Struct case - need more test data
                                Some(format!("{}{}", stack, uppercase_first_letter(key)))
                            },
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
                    format!("{}{}", stack, uppercase_first_letter(key))
                }
            }
            "string" => "String".to_string(),
            "boolean" => "bool".to_string(),
            "date" => extract_date_type(value)?,
            "number" => extract_number_type(value)?,
            "integer" => extract_integer_type(value)?,
            "array" => {
                // recurse through repeated arrays until we find a concrete type (keep track of how deep we went)
                let (array_type, recurse_level) = array_recurse_for_type(value, stack, key, 1)?;
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
        let member_doc = value.description.clone();
        if reqs.contains(key) {
            debug!("with required member {} of type {}", key, rust_type);
            members.push(OutputMember {
                type_: rust_type,
                name: key.to_string(),
                field_annot: None,
                docs: member_doc,
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
                    docs: member_doc,
                })
            } else if rust_type.starts_with("Vec") {
                members.push(OutputMember {
                    type_: rust_type,
                    name: key.to_string(),
                    field_annot: Some(r#"#[serde(default, skip_serializing_if = "Vec::is_empty")]"#.into()),
                    docs: member_doc,
                })
            } else {
                members.push(OutputMember {
                    type_: format!("Option<{}>", rust_type),
                    name: key.to_string(),
                    field_annot: None,
                    docs: member_doc,
                })
            }
        }
    }
    results.push(OutputStruct {
        name: stack.to_string(),
        members,
        level,
        docs: schema.description.clone(),
    });
    Ok(results)
}

// recurse into an array type to find its nested type
// this recursion is intialised and ended within a single step of the outer recursion
fn array_recurse_for_type(
    value: &JSONSchemaProps,
    stack: &str,
    key: &str,
    level: u8,
) -> Result<(String, u8)> {
    if let Some(items) = &value.items {
        match items {
            JSONSchemaPropsOrArray::Schema(s) => {
                let inner_array_type = s.type_.clone().unwrap_or_default();
                return match inner_array_type.as_ref() {
                    "object" => {
                        let structsuffix = uppercase_first_letter(key);
                        Ok((format!("Vec<{}{}>", stack, structsuffix), level))
                    }
                    "string" => Ok(("Vec<String>".into(), level)),
                    "boolean" => Ok(("Vec<bool>".into(), level)),
                    "date" => Ok((format!("Vec<{}>", extract_date_type(value)?), level)),
                    "number" => Ok((format!("Vec<{}>", extract_number_type(value)?), level)),
                    "integer" => Ok((format!("Vec<{}>", extract_integer_type(value)?), level)),
                    "array" => Ok(array_recurse_for_type(s, stack, key, level + 1)?),
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
    // TODO: byte / password here?
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
    // Think kubernetes go types just do signed ints, but set a minimum to zero..
    // rust will set uint, so emitting that when possbile
    Ok(if let Some(f) = &value.format {
        match f.as_ref() {
            "int8" => "i8".to_string(),
            "int16" => "i16".to_string(),
            "int32" => "i32".to_string(),
            "int64" => "i64".to_string(),
            "int128" => "i128".to_string(),
            "uint8" => "u8".to_string(),
            "uint16" => "u16".to_string(),
            "uint32" => "u32".to_string(),
            "uint64" => "u64".to_string(),
            "uint128" => "u128".to_string(),
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
