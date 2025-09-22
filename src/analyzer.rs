//! Deals entirely with schema analysis for the purpose of creating output structs + members
use crate::{Container, MapType, Member, Output};
use anyhow::{bail, Result};
use heck::ToUpperCamelCase;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    JSONSchemaProps, JSONSchemaPropsOrArray, JSONSchemaPropsOrBool, JSON,
};
use std::collections::{BTreeMap, HashMap};

const IGNORED_KEYS: [&str; 3] = ["metadata", "apiVersion", "kind"];

#[derive(Default)]
pub struct Config {
    pub no_condition: bool,
    pub no_object_reference: bool,
    pub map: MapType,
    pub relaxed: bool,
}

/// Scan a schema for structs and members, and recurse to find all structs
///
/// All found output structs will have its names prefixed by the kind it is for
pub fn analyze(schema: JSONSchemaProps, kind: &str, cfg: Config) -> Result<Output> {
    let mut res = Output::default();

    analyze_(&schema, "", kind, 0, &mut res, &cfg)?;
    Ok(res)
}

/// Scan a schema for structs and members, and recurse to find all structs
///
/// schema: root schema / sub schema
/// current: current key name (or empty string for first call) - must capitalize first letter
/// stack: stacked concat of kind + current_{n-1} + ... + current (used to create dedup names/types)
/// level: recursion level (start at 0)
/// results: multable list of generated structs (not deduplicated)
fn analyze_(
    schema: &JSONSchemaProps,
    current: &str,
    stack: &str,
    level: u8,
    results: &mut Output,
    cfg: &Config,
) -> Result<()> {
    let props = schema.properties.clone().unwrap_or_default();
    let mut array_recurse_level: HashMap<String, u8> = Default::default();

    let camel_cased_stack = &stack.to_upper_camel_case();

    // create a Container if we have a container type:
    //trace!("analyze_ with {} + {}", current, stack);
    if schema.type_.clone().unwrap_or_default() == "object" {
        // we can have additionalProperties XOR properties
        // https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#validation
        if let Some(JSONSchemaPropsOrBool::Schema(s)) = schema.additional_properties.as_ref() {
            let dict_type = s.type_.clone().unwrap_or_default();
            // object with additionalProperties == map
            if let Some(extra_props) = &s.properties {
                // map values is an object with properties
                debug!(
                    "Generating map struct for {} (under {})",
                    current, camel_cased_stack
                );
                let c = extract_container(
                    extra_props,
                    camel_cased_stack,
                    &mut array_recurse_level,
                    level,
                    schema,
                    cfg,
                )?;
                results.insert(c); // deduplicated insert
            } else if dict_type == "object" {
                // recurse to see if we eventually find properties
                debug!(
                    "Recursing into nested additional properties for {} (under {})",
                    current, camel_cased_stack
                );
                analyze_(s, current, camel_cased_stack, level, results, cfg)?;
            } else if !dict_type.is_empty() {
                warn!("not generating type {} - using {} map", current, dict_type);
                return Ok(()); // no members here - it'll be inlined
            }
        } else {
            // else, regular properties only
            debug!("Generating struct for {} (under {})", current, camel_cased_stack);
            // initial analysis of properties (we do not recurse here, we need to find members first)
            if props.is_empty() && schema.x_kubernetes_preserve_unknown_fields.unwrap_or(false) {
                warn!("not generating type {} - using map", current);
                return Ok(());
            }
            let c = extract_container(
                &props,
                camel_cased_stack,
                &mut array_recurse_level,
                level,
                schema,
                cfg,
            )?;
            results.insert(c); // deduplicated insert
        }
    }
    //trace!("full schema here: {}", serde_yaml::to_string(&schema).unwrap());

    // If the container has members, we recurse through these members to find more containers.
    // NB: find_containers initiates recursion **for this container** and will end up invoking this fn,
    // so that we can create the Container with its members (same fn, above) in one step.
    // Once the Container has been made, we drop down here and restarting the process for its members.
    //
    // again; additionalProperties XOR properties
    let extras = if let Some(JSONSchemaPropsOrBool::Schema(s)) = schema.additional_properties.as_ref() {
        let extra_props = s.properties.clone().unwrap_or_default();
        find_containers(
            &extra_props,
            camel_cased_stack,
            &mut array_recurse_level,
            level,
            schema,
            cfg,
        )?
    } else {
        // regular properties only
        find_containers(
            &props,
            camel_cased_stack,
            &mut array_recurse_level,
            level,
            schema,
            cfg,
        )?
    };
    results.extend(extras);

    Ok(())
}

/// Dive into passed properties
///
/// This will recursively invoke the analyzer from any new type that needs investigation.
/// Upon recursion, we concatenate container names (so they are always unique across the tree)
/// and bump the level to have a way to sort the containers by depth.
fn find_containers(
    props: &BTreeMap<String, JSONSchemaProps>,
    stack: &str,
    array_recurse_level: &mut HashMap<String, u8>,
    level: u8,
    schema: &JSONSchemaProps,
    cfg: &Config,
) -> Result<Output> {
    //trace!("finding containers in: {}", serde_yaml::to_string(&props)?);
    let mut results = Output::default();
    for (key, value) in props {
        if level == 0 && IGNORED_KEYS.contains(&(key.as_ref())) {
            debug!("not recursing into ignored {}", key); // handled elsewhere
            continue;
        }
        let next_key = key.to_upper_camel_case();
        let next_stack = format!("{}{}", stack, next_key);
        let value_type = value.type_.clone().unwrap_or_default();
        match value_type.as_ref() {
            "object" => {
                // objects, maps
                let mut handled_inner = false;
                if let Some(JSONSchemaPropsOrBool::Schema(s)) = &value.additional_properties {
                    let dict_type = s.type_.clone().unwrap_or_default();
                    if dict_type == "array" {
                        // unpack the inner object from the array wrap
                        if let Some(JSONSchemaPropsOrArray::Schema(items)) = &s.as_ref().items {
                            debug!("..recursing into object member {}", key);
                            analyze_(items, &next_key, &next_stack, level + 1, &mut results, cfg)?;
                            handled_inner = true;
                        }
                    }
                    // TODO: not sure if these nested recurses are necessary - cluster test case does not have enough data
                    //if let Some(extra_props) = &s.properties {
                    //    for (_key, value) in extra_props {
                    //        debug!("..nested recurse into {} {} - key: {}", next_key, next_stack, _key);
                    //        analyze_(value.clone(), &next_key, &next_stack, level +1, results)?;
                    //    }
                    //}
                }
                if !handled_inner {
                    // normal object recurse
                    analyze_(value, &next_key, &next_stack, level + 1, &mut results, cfg)?;
                }
            }
            "array" => {
                if let Some(recurse) = array_recurse_level.get(key).cloned() {
                    let mut inner = value.clone();
                    for _i in 0..recurse {
                        debug!("..recursing into props for {}", key);
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
                    analyze_(&inner, &next_key, &next_stack, level + 1, &mut results, cfg)?;
                }
            }
            "" => {
                if value.x_kubernetes_int_or_string.is_some() {
                    debug!("..not recursing into IntOrString {}", key)
                } else {
                    debug!("..not recursing into unknown empty type {}", key)
                }
            }
            x => {
                if let Some(en) = &value.enum_ {
                    // plain enums do not need to recurse, can collect it here
                    // ....although this makes it impossible for us to handle enums at the top level
                    // TODO: move this to the top level
                    let new_result = analyze_enum_properties(en, &next_stack, level, schema)?;
                    results.insert(new_result); // deduplicated insert
                } else {
                    debug!("..not recursing into {} ('{}' is not a container)", key, x)
                }
            }
        }
    }
    Ok(results)
}

// helper to figure out what output enums and embedded members are contained in the current object schema
fn analyze_enum_properties(
    items: &Vec<JSON>,
    stack: &str,
    level: u8,
    schema: &JSONSchemaProps,
) -> Result<Container, anyhow::Error> {
    let mut members = vec![];
    debug!("analyzing enum {}", serde_json::to_string(&schema).unwrap());
    for en in items {
        debug!("got enum {:?}", en);
        // TODO: do we need to verify enum elements? only in oneOf only right?
        let name = match &en.0 {
            serde_json::Value::String(name) => name.to_string(),
            serde_json::Value::Number(val) => {
                if !val.is_u64() {
                    bail!("enum member cannot have signed/floating discriminants");
                }
                val.to_string()
            }
            _ => bail!("not handling non-string/int enum outside oneOf block"),
        };
        let rust_type = "".to_string();
        // Create member and wrap types correctly
        let member_doc = None;
        debug!("with enum member {}", name);
        members.push(Member {
            type_: rust_type,
            name: name.to_string(),
            serde_annot: vec![],
            extra_annot: vec![],
            docs: member_doc,
        })
    }
    Ok(Container {
        name: stack.to_string(),
        members,
        level,
        docs: schema.description.clone(),
        is_enum: true,
        ..Container::default()
    })
}

// fully populate a Container with all its members given the current stack and schema position
fn extract_container(
    props: &BTreeMap<String, JSONSchemaProps>,
    stack: &str,
    array_recurse_level: &mut HashMap<String, u8>,
    level: u8,
    schema: &JSONSchemaProps,
    cfg: &Config,
) -> Result<Container, anyhow::Error> {
    let mut members = vec![];
    //debug!("analyzing object {}", serde_json::to_string(&schema).unwrap());
    let reqs = schema.required.clone().unwrap_or_default();
    for (key, value) in props {
        let value_type = value.type_.clone().unwrap_or_default();
        let rust_type = match value_type.as_ref() {
            "object" => extract_object_type(value, stack, key, cfg)?,
            "string" => {
                if let Some(_en) = &value.enum_ {
                    trace!("got enum string: {}", serde_json::to_string(&schema).unwrap());
                    format!("{}{}", stack, key.to_upper_camel_case())
                } else {
                    "String".to_string()
                }
            }
            "boolean" => "bool".to_string(),
            "date" => extract_date_type(value)?,
            "number" => extract_number_type(value)?,
            "integer" => extract_integer_type(value)?,
            "array" => {
                // recurse through repeated arrays until we find a concrete type (keep track of how deep we went)
                let (mut array_type, recurse_level) = array_recurse_for_type(value, stack, key, 1, cfg)?;
                trace!("got array {} for {} in level {}", array_type, key, recurse_level);
                if !cfg.no_condition && key == "conditions" && is_conditions(value) {
                    array_type = "Vec<Condition>".into();
                } else if !cfg.no_object_reference && is_object_ref_list(value) {
                    array_type = "Vec<ObjectReference>".into()
                } else {
                    array_recurse_level.insert(key.clone(), recurse_level);
                }
                array_type
            }
            "" => {
                let map_type = cfg.map.name();
                if value.x_kubernetes_int_or_string.is_some() {
                    "IntOrString".into()
                } else if value.x_kubernetes_preserve_unknown_fields == Some(true)
                    || value
                        .one_of
                        .as_deref()
                        .is_some_and(|items| items.iter().all(|item| item.type_.is_some()))
                {
                    "serde_json::Value".into()
                } else if cfg.relaxed {
                    debug!("found empty object at {} key: {}", stack, key);
                    format!("{map_type}<String, serde_json::Value>")
                } else {
                    bail!("unknown empty dict type for {}", key)
                }
            }
            x => bail!("unknown type {}", x),
        };

        // Create member and wrap types correctly
        let member_doc = value.description.clone();
        if reqs.contains(key) {
            debug!("with required member {} of type {}", key, &rust_type);
            members.push(Member {
                type_: rust_type,
                name: key.to_string(),
                serde_annot: vec![],
                extra_annot: vec![],
                docs: member_doc,
            })
        } else {
            // option wrapping needed if not required
            debug!("with optional member {} of type {}", key, rust_type);
            members.push(Member {
                type_: format!("Option<{}>", rust_type),
                name: key.to_string(),
                serde_annot: vec![
                    "default".into(),
                    "skip_serializing_if = \"Option::is_none\"".into(),
                ],
                extra_annot: vec![],
                docs: member_doc,
            })
            // TODO: must capture `default` key here instead of blindly using serde default
            // this will require us storing default properties for the member in above loop
            // This is complicated because serde default requires a default fn / impl Default
            // probably better to do impl Default to avoid having to make custom fns
        }
    }
    Ok(Container {
        name: stack.to_string(),
        members,
        level,
        docs: schema.description.clone(),
        is_enum: false,
        ..Container::default()
    })
}

fn resolve_additional_properties(
    additional: &JSONSchemaPropsOrBool,
    stack: &str,
    key: &str,
    cfg: &Config,
) -> Result<Option<String>, anyhow::Error> {
    debug!("got additional: {}", serde_json::to_string(&additional)?);
    let JSONSchemaPropsOrBool::Schema(s) = additional else {
        return Ok(None);
    };

    // This case is for maps. It is generally String -> Something, depending on the type key:
    let dict_type = s.type_.clone().unwrap_or_default();
    debug!("dict type is {dict_type}");
    let dict_key = match dict_type.as_ref() {
        "string" => Some("String".into()),
        // We are not 100% sure the array and object subcases here are correct but they pass tests atm.
        // authoratative, but more detailed sources than crd validation docs below are welcome
        // https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#validation
        "array" => Some(array_recurse_for_type(s, stack, key, 1, cfg)?.0),
        "object" => Some(extract_object_type(s, stack, key, cfg)?),
        "" => {
            if s.x_kubernetes_int_or_string.is_some() {
                Some("IntOrString".into())
            } else if s.x_kubernetes_preserve_unknown_fields == Some(true) {
                Some("serde_json::Value".into())
            } else {
                bail!("unknown empty dict type for {}", key)
            }
        }
        "boolean" => Some("bool".to_string()),
        "number" => Some(extract_number_type(s)?),
        "integer" => Some(extract_integer_type(s)?),
        // think the type we get is the value type
        x => Some(x.to_upper_camel_case()), // best guess
    };

    Ok(dict_key)
}

// recurse into an array type to find its nested type
// this recursion is intialised and ended within a single step of the outer recursion
fn array_recurse_for_type(
    value: &JSONSchemaProps,
    stack: &str,
    key: &str,
    level: u8,
    cfg: &Config,
) -> Result<(String, u8)> {
    if let Some(items) = &value.items {
        match items {
            JSONSchemaPropsOrArray::Schema(s) => {
                if s.type_.is_none() && s.x_kubernetes_preserve_unknown_fields == Some(true) {
                    return Ok(("Vec<serde_json::Value>".to_string(), level));
                }
                let inner_array_type = s.type_.clone().unwrap_or_default();
                match inner_array_type.as_ref() {
                    "object" => {
                        let vec_value = extract_object_type(s, stack, key, cfg)?;

                        Ok((format!("Vec<{}>", vec_value), level))
                    }
                    "string" => Ok(("Vec<String>".into(), level)),
                    "boolean" => Ok(("Vec<bool>".into(), level)),
                    "date" => Ok((format!("Vec<{}>", extract_date_type(value)?), level)),
                    "number" => Ok((format!("Vec<{}>", extract_number_type(value)?), level)),
                    "integer" => Ok((format!("Vec<{}>", extract_integer_type(value)?), level)),
                    "array" => {
                        if s.items.is_some() {
                            let (array_type, recurse_level) =
                                array_recurse_for_type(s, stack, key, level + 1, cfg)?;

                            Ok((format!("Vec<{}>", array_type), recurse_level))
                        } else if cfg.relaxed {
                            warn!("Empty inner array in: {} key: {}", stack, key);
                            let map_type = cfg.map.name();
                            Ok((format!("{}<String, serde_json::Value>", map_type), level))
                        } else {
                            bail!("Empty inner array in: {} key: {}", stack, key);
                        }
                    }
                    "" => {
                        if s.x_kubernetes_int_or_string.is_some() {
                            Ok(("Vec<IntOrString>".into(), level))
                        } else {
                            bail!("unknown empty array type for {}", key)
                        }
                    }
                    unknown => {
                        bail!("unsupported recursive array type \"{unknown}\" for {key}")
                    }
                }
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
fn is_conditions(value: &JSONSchemaProps) -> bool {
    if let Some(JSONSchemaPropsOrArray::Schema(props)) = &value.items {
        if let Some(p) = &props.properties {
            let type_ = p.get("type");
            let status = p.get("status");
            let reason = p.get("reason");
            let message = p.get("message");
            let ltt = p.get("lastTransitionTime");
            if type_.is_some() && status.is_some() && reason.is_some() && message.is_some() && ltt.is_some() {
                return true;
            }
        }
    }
    false
}

fn is_object_ref_list(value: &JSONSchemaProps) -> bool {
    if let Some(JSONSchemaPropsOrArray::Schema(props)) = &value.items {
        is_object_ref(props)
    } else {
        false
    }
}

fn is_object_ref(value: &JSONSchemaProps) -> bool {
    if let Some(p) = &value.properties {
        if p.len() != 7 {
            return false;
        }
        let api_version = p.get("apiVersion");
        let field_path = p.get("fieldPath");
        let kind = p.get("kind");
        let name = p.get("name");
        let ns = p.get("namespace");
        let rv = p.get("resourceVersion");
        let uid = p.get("uid");
        if [api_version, field_path, kind, name, ns, rv, uid]
            .iter()
            .all(|k| k.is_some())
        {
            return true;
        }
    }
    false
}

fn extract_object_type(
    value: &JSONSchemaProps,
    stack: &str,
    key: &str,
    cfg: &Config,
) -> Result<String, anyhow::Error> {
    let mut dict_key = None;
    if let Some(additional) = &value.additional_properties {
        dict_key = resolve_additional_properties(additional, stack, key, cfg)?;
    } else if value.properties.is_none() && value.x_kubernetes_preserve_unknown_fields.unwrap_or(false) {
        dict_key = Some("serde_json::Value".into());
    }
    Ok(if let Some(dict) = dict_key {
        format!("{}<String, {}>", cfg.map.name(), dict)
    } else if !cfg.no_object_reference && is_object_ref(value) {
        "ObjectReference".into()
    } else {
        format!("{}{}", stack, key.to_upper_camel_case())
    })
}

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
            _ => "f64".to_string(),
        }
    } else {
        "f64".to_string()
    })
}

fn extract_integer_type(value: &JSONSchemaProps) -> Result<String> {
    // Think kubernetes go types just do signed ints, but set a minimum to zero..
    // rust will set uint, so emitting that when possible
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
            _ => "i64".to_string(),
        }
    } else {
        "i64".to_string()
    })
}

// unit tests particular schema patterns
#[cfg(test)]
mod test {
    use super::{analyze, Config as Cfg};
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::JSONSchemaProps;

    use std::sync::Once;

    static START: Once = Once::new();
    fn init() {
        START.call_once(|| {
            env_logger::init();
        });
    }
    // To debug individual tests:
    // RUST_LOG=debug cargo test --lib -- --nocapture testname

    #[test]
    fn map_of_struct() {
        init();
        // validationsInfo from agent test
        let schema_str = r#"
        description: AgentStatus defines the observed state of Agent
        properties:
          validationsInfo:
            additionalProperties:
              items:
                properties:
                  id:
                    type: string
                  message:
                    type: string
                  status:
                    type: string
                required:
                - id
                - message
                - status
                type: object
              type: array
            description: ValidationsInfo is a JSON-formatted string containing
              the validation results for each validation id grouped by category
              (network, hosts-data, etc.)
            type: object
        type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        //println!("schema: {}", serde_json::to_string_pretty(&schema).unwrap());

        let structs = analyze(schema, "Agent", Cfg::default()).unwrap().output();
        //println!("{:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "Agent");
        assert_eq!(root.level, 0);
        // should have a member with a key to the map:
        let map = &root.members[0];
        assert_eq!(map.name, "validationsInfo");
        assert_eq!(map.type_, "Option<BTreeMap<String, Vec<AgentValidationsInfo>>>");
        // should have a separate struct
        let other = &structs[1];
        assert_eq!(other.name, "AgentValidationsInfo");
        assert_eq!(other.level, 1);
        assert_eq!(other.members[0].name, "id");
        assert_eq!(other.members[0].type_, "String");
        assert_eq!(other.members[1].name, "message");
        assert_eq!(other.members[1].type_, "String");
        assert_eq!(other.members[2].name, "status");
        assert_eq!(other.members[2].type_, "String");
    }

    #[test]
    fn map_of_map() {
        init();
        // as found in cnpg-cluster
        let schema_str = r#"
        description: Instances topology.
        properties:
          instances:
            additionalProperties:
              additionalProperties:
                type: string
              description: PodTopologyLabels represent the topology of a Pod.
                map[labelName]labelValue
              type: object
            description: Instances contains the pod topology of the instances
            type: object
        type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        //println!("schema: {}", serde_json::to_string_pretty(&schema).unwrap());

        let structs = analyze(schema, "ClusterStatusTopology", Cfg::default())
            .unwrap()
            .output();
        //println!("{:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "ClusterStatusTopology");
        assert_eq!(root.level, 0);
        // should have a member with a key to the map:
        let map = &root.members[0];
        assert_eq!(map.name, "instances");
        assert_eq!(map.type_, "Option<BTreeMap<String, BTreeMap<String, String>>>");
    }

    #[test]
    fn map_of_number() {
        init();
        // as found in aws-lambda-services-aliases
        let schema_str = r#"
        description: "The routing configuration (https://docs.aws.amazon.com/lambda/latest/dg/configuration-aliases.html#configuring-alias-routing)\nof the alias."
        properties:
          additionalVersionWeights:
            additionalProperties:
              type: "number"
            type: "object"
        type: "object"
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "AliasRoutingConfig", Cfg::default())
            .unwrap()
            .output(); //println!("{:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "AliasRoutingConfig");
        assert_eq!(root.level, 0);
        // should have a member with a key to the map:
        let map = &root.members[0];
        assert_eq!(map.name, "additionalVersionWeights");
        assert_eq!(map.type_, "Option<BTreeMap<String, f64>>");
    }

    #[test]
    fn empty_preserve_unknown_fields() {
        init();
        let schema_str = r#"
description: |-
  Identifies servers in the same namespace for which this authorization applies.
required:
  - selector
properties:
  selector:
    description: A label query over servers on which this authorization
      applies.
    required:
      - matchLabels
    properties:
      matchLabels:
        type: object
        x-kubernetes-preserve-unknown-fields: true
    type: object
type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        //println!("schema: {}", serde_json::to_string_pretty(&schema).unwrap());
        let structs = analyze(schema, "Server", Cfg::default()).unwrap().output();
        //println!("{:#?}", structs);

        let root = &structs[0];
        assert_eq!(root.name, "Server");
        assert_eq!(root.level, 0);
        let root_member = &root.members[0];
        assert_eq!(root_member.name, "selector");
        assert_eq!(root_member.type_, "ServerSelector");
        let server_selector = &structs[1];
        assert_eq!(server_selector.name, "ServerSelector");
        assert_eq!(server_selector.level, 1);
        let match_labels = &server_selector.members[0];
        assert_eq!(match_labels.name, "matchLabels");
        assert_eq!(match_labels.type_, "BTreeMap<String, serde_json::Value>");
    }

    #[test]
    fn additional_preserve_unknown() {
        init();
        let schema_str = r#"
    description: MiddlewareSpec defines the desired state of a Middleware.
    properties:
      plugin:
        additionalProperties:
          x-kubernetes-preserve-unknown-fields: true
        description: traefik middleware crd
        type: object
    type: object"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        println!("got {schema:?}");

        let structs = analyze(schema, "Spec", Cfg::default()).unwrap().output();
        println!("got: {structs:?}");
        let root = &structs[0];
        assert_eq!(root.name, "Spec");
        let member = &root.members[0];
        assert_eq!(member.name, "plugin");
        assert_eq!(member.type_, "Option<BTreeMap<String, serde_json::Value>>");
    }

    #[test]
    fn no_type_preserve_unknown_fields() {
        init();
        let schema_str = r#"
description: Schema defines the schema of the variable.
properties:
  openAPIV3Schema:
    description: |-
      OpenAPIV3Schema defines the schema of a variable via OpenAPI v3
      schema. The schema is a subset of the schema used in
      Kubernetes CRDs.
    properties:
      items:
        description: |-
          Items specifies fields of an array.
          NOTE: Can only be set if type is array.
          NOTE: This field uses PreserveUnknownFields and Schemaless,
          because recursive validation is not possible.
        x-kubernetes-preserve-unknown-fields: true
      requiredItems:
        description: |-
          Items specifies fields of an array.
          NOTE: Can only be set if type is array.
          NOTE: This field uses PreserveUnknownFields and Schemaless,
          because recursive validation is not possible.
        x-kubernetes-preserve-unknown-fields: true
    required:
    - requiredItems
    type: object
required:
- openAPIV3Schema
type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        // println!("schema: {}", serde_json::to_string_pretty(&schema).unwrap());
        let structs = analyze(schema, "Variables", Cfg::default()).unwrap().output();
        // println!("{:#?}", structs);

        let root = &structs[0];
        assert_eq!(root.name, "Variables");
        assert_eq!(root.level, 0);
        let root_member = &root.members[0];
        assert_eq!(root_member.name, "openAPIV3Schema");
        assert_eq!(root_member.type_, "VariablesOpenApiv3Schema");
        let variables_schema = &structs[1];
        assert_eq!(variables_schema.name, "VariablesOpenApiv3Schema");
        assert_eq!(variables_schema.level, 1);
        let items = &variables_schema.members[0];
        assert_eq!(items.name, "items");
        assert_eq!(items.type_, "Option<serde_json::Value>");
        let required_items = &variables_schema.members[1];
        assert_eq!(required_items.name, "requiredItems");
        assert_eq!(required_items.type_, "serde_json::Value");
    }

    #[test]
    fn int_or_string() {
        init();
        let schema_str = r#"
            properties:
              port:
                description: A port name or number. Must exist in a pod spec.
                x-kubernetes-int-or-string: true
            required:
            - port
            type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Server", Cfg::default()).unwrap().output();
        let root = &structs[0];
        assert_eq!(root.name, "Server");
        // should have an IntOrString member:
        let member = &root.members[0];
        assert_eq!(member.name, "port");
        assert_eq!(member.type_, "IntOrString");
        assert!(root.uses_int_or_string());
        // TODO: check that anyOf: [type: integer, type: string] also works
    }

    #[test]
    fn string_or_string_array() {
        init();
        let schema_str = r#"
            type: object
            required:
              - ambassadorId
            properties:
              ambassadorId:
                items:
                  type: "string"
                oneOf:
                  - type: "string"
                  - type: "array"
              other:
                items:
                  type: "string"
                oneOf:
                  - type: "string"
                  - type: "array"
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Host", Cfg::default()).unwrap().output();

        let root = &structs[0];
        assert_eq!(root.name, "Host");

        let member = &root.members[0];
        assert_eq!(member.name, "ambassadorId");
        assert_eq!(member.type_, "serde_json::Value");

        let member = &root.members[1];
        assert_eq!(member.name, "other");
        assert_eq!(member.type_, "Option<serde_json::Value>");
    }

    #[test]
    fn boolean_in_additionals() {
        // as found in argo-app
        init();
        let schema_str = r#"
            properties:
              options:
                additionalProperties:
                  type: boolean
                type: object
              patch:
                type: string
            type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "Options", Cfg::default()).unwrap().output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "Options");
        assert_eq!(root.level, 0);
        assert_eq!(&root.members[0].name, "options");
        assert_eq!(&root.members[0].type_, "Option<BTreeMap<String, bool>>");
    }

    #[test]
    fn enum_string() {
        init();
        let schema_str = r#"
      properties:
        operator:
          enum:
          - In
          - NotIn
          - Exists
          - DoesNotExist
          type: string
      required:
      - operator
      type: object
"#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "MatchExpressions", Cfg::default())
            .unwrap()
            .output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "MatchExpressions");
        assert_eq!(root.level, 0);
        assert_eq!(&root.members[0].name, "operator");
        assert_eq!(&root.members[0].type_, "MatchExpressionsOperator");

        // operator member
        let op = &structs[1];
        assert!(op.is_enum);
        assert_eq!(op.name, "MatchExpressionsOperator");

        // should have enum members:
        assert_eq!(&op.members[0].name, "In");
        assert_eq!(&op.members[0].type_, "");
        assert_eq!(&op.members[1].name, "NotIn");
        assert_eq!(&op.members[1].type_, "");
        assert_eq!(&op.members[2].name, "Exists");
        assert_eq!(&op.members[2].type_, "");
        assert_eq!(&op.members[3].name, "DoesNotExist");
        assert_eq!(&op.members[3].type_, "");
    }

    #[test]
    fn enum_string_within_container() {
        init();
        let schema_str = r#"
      description: Endpoint
      properties:
        relabelings:
          items:
            properties:
              action:
                default: replace
                enum:
                - replace
                - keep
                - drop
                - hashmod
                - labelmap
                - labeldrop
                - labelkeep
                type: string
              modulus:
                format: int64
                type: integer
            type: object
          type: array
      type: object
        "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "Endpoint", Cfg::default()).unwrap().output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "Endpoint");
        assert_eq!(root.level, 0);
        assert!(!root.is_enum);
        assert_eq!(&root.members[0].name, "relabelings");
        assert_eq!(&root.members[0].type_, "Option<Vec<EndpointRelabelings>>");

        let rel = &structs[1];
        assert_eq!(rel.name, "EndpointRelabelings");
        assert!(!rel.is_enum);
        assert_eq!(&rel.members[0].name, "action");
        assert_eq!(&rel.members[0].type_, "Option<EndpointRelabelingsAction>");
        // TODO: verify rel.members[0].field_annot uses correct default

        // action enum member
        let act = &structs[2];
        assert_eq!(act.name, "EndpointRelabelingsAction");
        assert!(act.is_enum);

        // should have enum members:
        assert_eq!(&act.members[0].name, "replace");
        assert_eq!(&act.members[0].type_, "");
        assert_eq!(&act.members[1].name, "keep");
        assert_eq!(&act.members[1].type_, "");
        assert_eq!(&act.members[2].name, "drop");
        assert_eq!(&act.members[2].type_, "");
        assert_eq!(&act.members[3].name, "hashmod");
        assert_eq!(&act.members[3].type_, "");
    }

    #[test]
    #[ignore] // oneof support not done
    fn enum_oneof() {
        init();
        let schema_str = r#"
    description: "Auto-generated derived type for ServerSpec via `CustomResource`"
    properties:
      spec:
        properties:
          podSelector:
            oneOf:
              - required:
                  - matchExpressions
              - required:
                  - matchLabels
            properties:
              matchExpressions:
                items:
                  properties:
                    key:
                      type: string
                    operator:
                      enum:
                        - In
                        - NotIn
                        - Exists
                        - DoesNotExists
                      type: string
                    values:
                      items:
                        type: string
                      nullable: true
                      type: array
                  required:
                    - key
                    - operator
                  type: object
                type: array
              matchLabels:
                additionalProperties:
                  type: string
                type: object
            type: object
        required:
          - podSelector
        type: object
    required:
      - spec
    title: Server
    type: object"#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "ServerSpec", Cfg::default()).unwrap().output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "ServerSpec");
        assert_eq!(root.level, 0);

        // should have a required selector
        let member = &root.members[0];
        assert_eq!(member.name, "pod_selector");
        assert_eq!(member.type_, "ServerPodSelector");

        // and this should be an enum
        let ps = &structs[1]; // TODO: encode as struct?
        assert_eq!(ps.name, "ServerPodSelector");
        assert_eq!(ps.level, 1);

        // should have enum members: TODO: encode inner type as type_?
        assert_eq!(&ps.members[0].name, "MatchExpressions");
        assert_eq!(&ps.members[0].type_, "Vec<ServerPodSelectorMatchExpressions");
        assert_eq!(&ps.members[1].name, "MatchLabels");
        assert_eq!(&ps.members[1].type_, "BTreeMap<String, String>");

        // should have the inner struct match expressions
        let me = &structs[2];
        assert_eq!(me.name, "ServerPodSelectorMatchExpressions");
        assert_eq!(me.level, 2);

        // which should have 3 members
        assert_eq!(&me.members[0].name, "key");
        assert_eq!(&me.members[0].type_, "String");
        assert_eq!(&me.members[1].name, "operator");
        assert_eq!(&me.members[1].type_, "ServerPodSelectorMatchExpressionsOperator");
        assert_eq!(&me.members[2].name, "values");
        assert_eq!(&me.members[2].type_, " Option<Vec<String>>");

        // last struct being the innermost enum operator:
        let op = &structs[3];
        assert_eq!(op.name, "ServerPodSelectorMatchExpressionsOperator");
        assert_eq!(op.level, 3);

        // with enum members:
        assert_eq!(&op.members[0].name, "In");
        assert_eq!(&op.members[1].name, "In");
        assert_eq!(&op.members[2].name, "In");
        assert_eq!(&op.members[3].name, "In");
    }

    #[test]
    fn service_monitor_params() {
        init();
        let schema_str = r#"
        properties:
          endpoints:
            items:
              description: Endpoint defines a scrapeable endpoint serving Prometheus
                metrics.
              properties:
                params:
                  additionalProperties:
                    items:
                      type: string
                    type: array
                  description: Optional HTTP URL parameters
                  type: object
              type: object
            type: array
        required:
        - endpoints
        type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "ServiceMonitor", Cfg::default())
            .unwrap()
            .output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "ServiceMonitor");
        assert_eq!(root.level, 0);

        // should have a required endpoints member
        let member = &root.members[0];
        assert_eq!(member.name, "endpoints");
        assert_eq!(member.type_, "Vec<ServiceMonitorEndpoints>");

        // Should have a endpoints struct:
        let eps = &structs[1];
        assert_eq!(eps.name, "ServiceMonitorEndpoints");
        assert_eq!(eps.level, 1);
        // should have an params member:
        let member = &eps.members[0];
        assert_eq!(member.name, "params");
        assert_eq!(member.type_, "Option<BTreeMap<String, Vec<String>>>");
    }

    #[test]
    fn integer_handling_in_maps() {
        init();
        // via https://istio.io/latest/docs/reference/config/networking/destination-rule/
        // distribute:
        // - from: us-west/zone1/*
        //   to:
        //     "us-west/zone1/*": 80
        //     "us-west/zone2/*": 20
        // - from: us-west/zone2/*
        //   to:
        //     "us-west/zone1/*": 20
        //     "us-west/zone2/*": 80

        // i.e. distribute is an array of {from: String, to: BTreeMap<String, Integer>}
        // with the correct integer type

        // the schema is found in destinationrule-crd.yaml with this excerpt:
        let schema_str = r#"
        properties:
          distribute:
            description: 'Optional: only one of distribute, failover
              or failoverPriority can be set.'
            items:
              properties:
                from:
                  description: Originating locality, '/' separated
                  type: string
                to:
                  additionalProperties:
                    type: integer
                    format: int32
                  description: Map of upstream localities to traffic
                    distribution weights.
                  type: object
              type: object
            type: array
        type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        //println!("schema: {}", serde_json::to_string_pretty(&schema).unwrap());
        let structs = analyze(schema, "DestinationRule", Cfg::default())
            .unwrap()
            .output();
        //println!("{:#?}", structs);

        // this should produce the root struct struct
        let root = &structs[0];
        assert_eq!(root.name, "DestinationRule");
        // which contains the distribute member:
        let distmember = &root.members[0];
        assert_eq!(distmember.name, "distribute");
        assert_eq!(distmember.type_, "Option<Vec<DestinationRuleDistribute>>");
        // which references the map type with {from,to} so find that struct:
        let ruledist = &structs[1];
        assert_eq!(ruledist.name, "DestinationRuleDistribute");
        // and has from and to members
        let from = &ruledist.members[0];
        let to = &ruledist.members[1];
        assert_eq!(from.name, "from");
        assert_eq!(to.name, "to");
        assert_eq!(from.type_, "Option<String>");
        assert_eq!(to.type_, "Option<BTreeMap<String, i32>>");
    }

    #[test]
    #[ignore] // currently do not handle top level enums, and this has an integration test
    fn top_level_enum_with_integers() {
        init();
        let schema_str = r#"
        default: 302
        enum:
        - 301
        - 302
        type: integer
        "#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        println!("got schema {}", serde_yaml::to_string(&schema).unwrap());
        let structs = analyze(schema, "StatusCode", Cfg::default()).unwrap().output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "StatusCode");
        assert_eq!(root.level, 0);
        assert!(root.is_enum);
        assert_eq!(&root.members[0].name, "301");
        assert_eq!(&root.members[0].name, "302");
        assert_eq!(&root.members[0].type_, "");
    }

    #[test]
    fn array_of_preserve_unknown_objects() {
        init();
        // example from flux kustomization crd
        let schema_str = r#"
        properties:
          patchesStrategicMerge:
            description: Strategic merge patches, defined as inline YAML objects.
            items:
              x-kubernetes-preserve-unknown-fields: true
            type: array
        type: object
        "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "KustomizationSpec", Cfg::default())
            .unwrap()
            .output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "KustomizationSpec");
        assert_eq!(root.level, 0);
        assert!(!root.is_enum);
        assert_eq!(&root.members[0].name, "patchesStrategicMerge");
        assert_eq!(&root.members[0].type_, "Option<Vec<serde_json::Value>>");
    }

    #[test]
    fn array_of_int_or_strings() {
        init();
        let schema_str = r#"
        properties:
          targetPorts:
            description: Numbers or names of the port to access on the pods targeted by the service.
            items:
              x-kubernetes-int-or-string: true
            type: array
        type: object
        "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "Schema", Cfg::default()).unwrap().output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "Schema");
        assert_eq!(root.level, 0);
        assert!(!root.is_enum);
        assert_eq!(&root.members[0].name, "targetPorts");
        assert_eq!(&root.members[0].type_, "Option<Vec<IntOrString>>");
    }

    #[test]
    fn nested_properties_in_additional_properties() {
        init();
        // example from flux kustomization crd
        let schema_str = r#"
        properties:
          jwtTokensByRole:
            additionalProperties:
              description: JWTTokens represents a list of JWT tokens
              properties:
                items:
                  items:
                    properties:
                      exp:
                        format: int64
                        type: integer
                      iat:
                        format: int64
                        type: integer
                      id:
                        type: string
                    required:
                    - iat
                    type: object
                  type: array
              type: object
            type: object
        type: object"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();
        let structs = analyze(schema, "AppProjectStatus", Cfg::default())
            .unwrap()
            .output();
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "AppProjectStatus");
        assert_eq!(root.level, 0);
        assert!(!root.is_enum);
        assert_eq!(&root.members[0].name, "jwtTokensByRole");
        assert_eq!(
            &root.members[0].type_,
            "Option<BTreeMap<String, AppProjectStatusJwtTokensByRole>>"
        );
        let role = &structs[1];
        assert_eq!(role.level, 1);
        assert_eq!(role.name, "AppProjectStatusJwtTokensByRole");
        assert_eq!(&role.members[0].name, "items");
        let items = &structs[2];
        assert_eq!(items.level, 2);
        assert_eq!(items.name, "AppProjectStatusJwtTokensByRoleItems");
        assert_eq!(&items.members[0].name, "exp");
        assert_eq!(&items.members[1].name, "iat");
        assert_eq!(&items.members[2].name, "id");
    }

    #[test]
    fn underscore_to_camel_case() {
        init();
        let schema_str = r#"
        properties:
          validations_info:
            type: object
        type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Agent", Cfg::default()).unwrap().output();

        let root = &structs[0];
        assert_eq!(root.name, "Agent");
        assert_eq!(root.level, 0);

        let map = &root.members[0];
        assert_eq!(map.name, "validations_info");
        assert_eq!(map.type_, "Option<AgentValidationsInfo>");

        let other = &structs[1];
        assert_eq!(other.name, "AgentValidationsInfo");
        assert_eq!(other.level, 1);
    }

    #[test]
    fn camel_case_of_kinds_with_consecutive_upper_case_letters() {
        init();
        let schema_str = r#"
        properties:
          spec:
            type: object
          status:
            type: object
        type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "ArgoCDExport", Cfg::default()).unwrap().output();

        let root = &structs[0];
        assert_eq!(root.name, "ArgoCdExport");
        assert_eq!(root.level, 0);

        let spec = &structs[1];
        assert_eq!(spec.name, "ArgoCdExportSpec");
        assert_eq!(spec.level, 1);

        let status = &structs[2];
        assert_eq!(status.name, "ArgoCdExportStatus");
        assert_eq!(status.level, 1);
    }

    #[test]
    fn skipped_type_as_map_nested_in_array() {
        init();
        let schema_str = r#"
properties:
  records:
    items:
      additionalProperties:
        type: string
      type: object
    type: array
type: object
"#;
        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Geoip", Cfg::default()).unwrap().output();

        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].members.len(), 1);
        assert_eq!(structs[0].members[0].name, "records");
        assert_eq!(
            structs[0].members[0].type_,
            "Option<Vec<BTreeMap<String, String>>>"
        );
    }

    #[test]
    fn uses_k8s_openapi_conditions() {
        init();
        let schema_str = r#"
properties:
  conditions:
    items:
      properties:
        lastTransitionTime:
          type: string  
        message:
          type: string  
        observedGeneration:
          type: integer
        reason:
          type: string
        status:
          type: string
        type:
          type: string
      type: object
    type: array
type: object
"#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Gateway", Cfg::default()).unwrap().output();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].members.len(), 1);
        assert_eq!(structs[0].members[0].type_, "Option<Vec<Condition>>");
    }

    #[test]
    fn uses_k8s_openapi_object_reference() {
        init();
        let schema_str = r#"
properties:
  myRef:
    properties:
      apiVersion:
        type: string
      fieldPath:
        type: string
      kind:
        type: string
      name:
        type: string
      namespace:
        type: string
      resourceVersion:
        type: string
      uid:
        type: string
    type: object
type: object
"#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Reference", Cfg::default()).unwrap().output();
        assert_eq!(structs[0].members[0].type_, "Option<ObjectReference>");
    }

    #[test]
    fn uses_k8s_openapi_object_reference_in_list() {
        init();
        let schema_str = r#"
        properties:
          controlPlaneRef:
            items:
              properties:
                apiVersion:
                  type: string
                fieldPath:
                  type: string
                kind:
                  type: string
                name:
                  type: string
                namespace:
                  type: string
                resourceVersion:
                  type: string
                uid:
                  type: string
              type: object
              x-kubernetes-map-type: atomic
            type: array
        type: object
        "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "Reference", Cfg::default()).unwrap().output();
        assert_eq!(structs[0].members[0].type_, "Option<Vec<ObjectReference>>");
    }

    #[test]
    fn lowercase_kind() {
        init();

        let schema_str = r#"
        properties:
          prop:
            type: object
        required:
        - prop
        type: object
        "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "postgresql", Cfg::default()).unwrap().output();
        assert_eq!(structs[0].members[0].type_, "PostgresqlProp");
    }

    #[test]
    fn nested_additional_properties_object() {
        init();

        // as found in rook-cephcluster
        let schema_str = r#"
        properties:
          cephConfigFromSecret:
            additionalProperties:
              additionalProperties:
                description: "SecretKeySelector selects a key of a Secret."
                properties:
                  key:
                    description: "The key of the secret to select from.  Must be a valid secret key."
                    type: "string"
                  name:
                    default: ""
                    description: "Name of the referent.\nThis field is effectively required, but due to backwards compatibility is\nallowed to be empty. Instances of this type with an empty value here are\nalmost certainly wrong.\nMore info: https://kubernetes.io/docs/concepts/overview/working-with-objects/names/#names"
                    type: "string"
                  optional:
                    description: "Specify whether the Secret or its key must be defined"
                    type: "boolean"
                required:
                  - "key"
                type: "object"
                x-kubernetes-map-type: "atomic"
              type: "object"
            description: "CephConfigFromSecret works exactly like CephConfig but takes config value from Secret Key reference."
            nullable: true
            type: "object"
        type: object
        "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "CephClusterSpec", Cfg::default())
            .unwrap()
            .output();

        // debug!("got: {:#?}", structs);

        assert_eq!(structs.len(), 2);
        assert_eq!(
            structs[0].members[0].type_,
            "Option<BTreeMap<String, BTreeMap<String, CephClusterSpecCephConfigFromSecret>>>"
        );
        assert_eq!(structs[1].name, "CephClusterSpecCephConfigFromSecret");
    }

    #[test]
    fn array_of_array() {
        init();
        let schema_str = r#"
      properties:
        mounts:
          description: mounts specifies a list of mount points to be setup.
          items:
            description: MountPoints defines input for generated mounts in cloud-init.
            items:
              type: string
            type: array
          type: array
      type: object
      "#;

        let schema: JSONSchemaProps = serde_yaml::from_str(schema_str).unwrap();

        let structs = analyze(schema, "KubeadmConfig", Cfg::default()).unwrap().output();

        let root = &structs[0];
        assert_eq!(root.name, "KubeadmConfig");
        assert_eq!(root.level, 0);

        let map = &root.members[0];
        assert_eq!(map.name, "mounts");
        assert_eq!(map.type_, "Option<Vec<Vec<String>>>");
    }
}
