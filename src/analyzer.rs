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
    pub map: MapType,
    pub relaxed: bool,
}

/// Scan a schema for structs and members, and recurse to find all structs
///
/// All found output structs will have its names prefixed by the kind it is for
pub fn analyze(schema: JSONSchemaProps, kind: &str, cfg: Config) -> Result<Output> {
    let mut res = vec![];
    analyze_(&schema, "", kind, 0, &mut res, &cfg)?;
    Ok(Output(res))
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
    results: &mut Vec<Container>,
    cfg: &Config,
) -> Result<()> {
    let props = schema.properties.clone().unwrap_or_default();
    let mut array_recurse_level: HashMap<String, u8> = Default::default();

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
                debug!("Generating map struct for {} (under {})", current, stack);
                let c = extract_container(extra_props, stack, &mut array_recurse_level, level, schema, cfg)?;
                results.push(c);
            } else if !dict_type.is_empty() {
                warn!("not generating type {} - using {} map", current, dict_type);
                return Ok(()); // no members here - it'll be inlined
            }
        } else {
            // else, regular properties only
            debug!("Generating struct for {} (under {})", current, stack);
            // initial analysis of properties (we do not recurse here, we need to find members first)
            if props.is_empty() && schema.x_kubernetes_preserve_unknown_fields.unwrap_or(false) {
                warn!("not generating type {} - using map", current);
                return Ok(());
            }
            let c = extract_container(&props, stack, &mut array_recurse_level, level, schema, cfg)?;
            results.push(c);
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
        find_containers(&extra_props, stack, &mut array_recurse_level, level, schema, cfg)?
    } else {
        // regular properties only
        find_containers(&props, stack, &mut array_recurse_level, level, schema, cfg)?
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
) -> Result<Vec<Container>> {
    //trace!("finding containers in: {}", serde_yaml::to_string(&props)?);
    let mut results = vec![];
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
                    results.push(new_result);
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
            "object" => {
                let mut dict_key = None;
                if let Some(additional) = &value.additional_properties {
                    dict_key = resolve_additional_properties(additional, stack, key, value)?;
                } else if value.properties.is_none()
                    && value.x_kubernetes_preserve_unknown_fields.unwrap_or(false)
                {
                    dict_key = Some("serde_json::Value".into());
                }
                if let Some(dict) = dict_key {
                    format!("{}<String, {}>", cfg.map.name(), dict)
                } else {
                    format!("{}{}", stack, key.to_upper_camel_case())
                }
            }
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
                } else {
                    array_recurse_level.insert(key.clone(), recurse_level);
                }
                array_type
            }
            "" => {
                let map_type = cfg.map.name();
                if value.x_kubernetes_int_or_string.is_some() {
                    "IntOrString".into()
                } else if value.x_kubernetes_preserve_unknown_fields == Some(true) {
                    format!("{map_type}<String, serde_json::Value>")
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
    })
}

fn resolve_additional_properties(
    additional: &JSONSchemaPropsOrBool,
    stack: &str,
    key: &str,
    value: &JSONSchemaProps,
) -> Result<Option<String>, anyhow::Error> {
    debug!("got additional: {}", serde_json::to_string(&additional)?);
    let JSONSchemaPropsOrBool::Schema(s) = additional else {
        return Ok(None);
    };

    // This case is for maps. It is generally String -> Something, depending on the type key:
    let dict_type = s.type_.clone().unwrap_or_default();
    let dict_key = match dict_type.as_ref() {
        "string" => Some("String".into()),
        // We are not 100% sure the array and object subcases here are correct but they pass tests atm.
        // authoratative, but more detailed sources than crd validation docs below are welcome
        // https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#validation
        "array" => {
            let mut simple_inner = None;
            if let Some(JSONSchemaPropsOrArray::Schema(ix)) = &s.items {
                simple_inner = ix.type_.clone();
                debug!("additional simple inner  type: {:?}", simple_inner);
            }
            // Simple case: additionalProperties contain: {items: {type: K}}
            // Then it's a simple map (service_monitor_params) - but key is useless
            match simple_inner.as_deref() {
                Some("string") => Some("String".into()),
                Some("integer") => Some(extract_integer_type(s)?),
                Some("date") => Some(extract_date_type(value)?),
                Some("") => {
                    if s.x_kubernetes_int_or_string.is_some() {
                        Some("IntOrString".into())
                    } else {
                        bail!("unknown inner empty dict type for {}", key)
                    }
                }
                // can probably cover the regulars here as well

                // Harder case: inline structs under items (agent test with `validationInfo`)
                // key becomes the struct
                Some("object") => Some(format!("{}{}", stack, key.to_upper_camel_case())),
                None => Some(format!("{}{}", stack, key.to_upper_camel_case())),

                // leftovers, array of arrays?... need a better way to recurse probably
                Some(x) => bail!("unknown inner empty dict type {} for {}", x, key),
            }
        }
        "object" => {
            // cluster test with `failureDomains` uses this spec format
            Some(format!("{}{}", stack, key.to_upper_camel_case()))
        }
        "" => {
            if s.x_kubernetes_int_or_string.is_some() {
                Some("IntOrString".into())
            } else {
                bail!("unknown empty dict type for {}", key)
            }
        }
        "boolean" => Some("bool".to_string()),
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
                    let map_type = cfg.map.name();
                    return Ok((format!("Vec<{}<String, serde_json::Value>>", map_type), level));
                }
                let inner_array_type = s.type_.clone().unwrap_or_default();
                return match inner_array_type.as_ref() {
                    "object" => {
                        // Same logic as in `extract_container` to simplify types to maps.
                        let mut dict_value = None;
                        if let Some(additional) = &s.additional_properties {
                            dict_value = resolve_additional_properties(additional, stack, key, s)?;
                        }

                        let vec_value = if let Some(dict_value) = dict_value {
                            let map_type = cfg.map.name();
                            format!("{map_type}<String, {dict_value}>")
                        } else {
                            let structsuffix = key.to_upper_camel_case();
                            format!("{stack}{structsuffix}")
                        };

                        Ok((format!("Vec<{}>", vec_value), level))
                    }
                    "string" => Ok(("Vec<String>".into(), level)),
                    "boolean" => Ok(("Vec<bool>".into(), level)),
                    "date" => Ok((format!("Vec<{}>", extract_date_type(value)?), level)),
                    "number" => Ok((format!("Vec<{}>", extract_number_type(value)?), level)),
                    "integer" => Ok((format!("Vec<{}>", extract_integer_type(value)?), level)),
                    "array" => {
                        if s.items.is_some() {
                            Ok(array_recurse_for_type(s, stack, key, level + 1, cfg)?)
                        } else if cfg.relaxed {
                            warn!("Empty inner array in: {} key: {}", stack, key);
                            let map_type = cfg.map.name();
                            Ok((format!("{}<String, serde_json::Value>", map_type), level))
                        } else {
                            bail!("Empty inner array in: {} key: {}", stack, key);
                        }
                    }
                    unknown => {
                        bail!("unsupported recursive array type \"{unknown}\" for {key}")
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

        let structs = analyze(schema, "Agent", Cfg::default()).unwrap().0;
        //println!("{:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "Agent");
        assert_eq!(root.level, 0);
        // should have a member with a key to the map:
        let map = &root.members[0];
        assert_eq!(map.name, "validationsInfo");
        assert_eq!(map.type_, "Option<BTreeMap<String, AgentValidationsInfo>>");
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
        let structs = analyze(schema, "Server", Cfg::default()).unwrap().0;
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

        let structs = analyze(schema, "Server", Cfg::default()).unwrap().0;
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
        let structs = analyze(schema, "Options", Cfg::default()).unwrap().0;
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
        let structs = analyze(schema, "MatchExpressions", Cfg::default()).unwrap().0;
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
        let structs = analyze(schema, "Endpoint", Cfg::default()).unwrap().0;
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "Endpoint");
        assert_eq!(root.level, 0);
        assert_eq!(root.is_enum, false);
        assert_eq!(&root.members[0].name, "relabelings");
        assert_eq!(&root.members[0].type_, "Option<Vec<EndpointRelabelings>>");

        let rel = &structs[1];
        assert_eq!(rel.name, "EndpointRelabelings");
        assert_eq!(rel.is_enum, false);
        assert_eq!(&rel.members[0].name, "action");
        assert_eq!(&rel.members[0].type_, "Option<EndpointRelabelingsAction>");
        // TODO: verify rel.members[0].field_annot uses correct default

        // action enum member
        let act = &structs[2];
        assert_eq!(act.name, "EndpointRelabelingsAction");
        assert_eq!(act.is_enum, true);

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
        let structs = analyze(schema, "ServerSpec", Cfg::default()).unwrap().0;
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
        let structs = analyze(schema, "ServiceMonitor", Cfg::default()).unwrap().0;
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
        assert_eq!(member.type_, "Option<BTreeMap<String, String>>");
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
        let structs = analyze(schema, "DestinationRule", Cfg::default()).unwrap().0;
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
        let structs = analyze(schema, "StatusCode", Cfg::default()).unwrap().0;
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "StatusCode");
        assert_eq!(root.level, 0);
        assert_eq!(root.is_enum, true);
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
        let structs = analyze(schema, "KustomizationSpec", Cfg::default()).unwrap().0;
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "KustomizationSpec");
        assert_eq!(root.level, 0);
        assert_eq!(root.is_enum, false);
        assert_eq!(&root.members[0].name, "patchesStrategicMerge");
        assert_eq!(
            &root.members[0].type_,
            "Option<Vec<BTreeMap<String, serde_json::Value>>>"
        );
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
        let structs = analyze(schema, "AppProjectStatus", Cfg::default()).unwrap().0;
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "AppProjectStatus");
        assert_eq!(root.level, 0);
        assert_eq!(root.is_enum, false);
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

        let structs = analyze(schema, "Agent", Cfg::default()).unwrap().0;

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

        let structs = analyze(schema, "Geoip", Cfg::default()).unwrap().0;

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

        let structs = analyze(schema, "Gateway", Cfg::default()).unwrap().0;
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].members.len(), 1);
        assert_eq!(structs[0].members[0].type_, "Option<Vec<Condition>>");
    }
}
