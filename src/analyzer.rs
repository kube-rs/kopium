//! Deals entirely with schema analysis for the purpose of creating output structs + members
use crate::{Container, Member, Output};
use anyhow::{bail, Result};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    JSONSchemaProps, JSONSchemaPropsOrArray, JSONSchemaPropsOrBool, JSON,
};
use std::collections::{BTreeMap, HashMap};

const IGNORED_KEYS: [&str; 3] = ["metadata", "apiVersion", "kind"];

/// Scan a schema for structs and members, and recurse to find all structs
///
/// All found output structs will have its names prefixed by the kind it is for
pub fn analyze(schema: JSONSchemaProps, kind: &str) -> Result<Output> {
    let mut res = vec![];
    analyze_(schema, "", kind, 0, &mut res)?;
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
    schema: JSONSchemaProps,
    current: &str,
    stack: &str,
    level: u8,
    results: &mut Vec<Container>,
) -> Result<()> {
    let props = schema.properties.clone().unwrap_or_default();
    let mut array_recurse_level: HashMap<String, u8> = Default::default();
    // first generate the object if it is one
    let current_type = schema.type_.clone().unwrap_or_default();
    if current_type == "object" {
        // we can have additionalProperties XOR properties
        // https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#validation
        if let Some(JSONSchemaPropsOrBool::Schema(s)) = schema.additional_properties.as_ref() {
            let dict_type = s.type_.clone().unwrap_or_default();
            // object with additionalProperties == map
            if let Some(extra_props) = &s.properties {
                // map values is an object with properties
                debug!("Generating map struct for {} (under {})", current, stack);
                let new_result =
                    analyze_object_properties(extra_props, stack, &mut array_recurse_level, level, &schema)?;
                results.extend(new_result);
            } else if !dict_type.is_empty() {
                warn!("not generating type {} - using {} map", current, dict_type);
                return Ok(()); // no members here - it'll be inlined
            }
        } else {
            // else, regular properties only
            debug!("Generating struct for {} (under {})", current, stack);
            // initial analysis of properties (we do not recurse here, we need to find members first)
            if props.is_empty() && schema.x_kubernetes_preserve_unknown_fields.unwrap_or(false) {
                warn!("not generating type {} - using BTreeMap", current);
                return Ok(());
            }
            let new_result =
                analyze_object_properties(&props, stack, &mut array_recurse_level, level, &schema)?;
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
                // objects, maps
                let mut handled_inner = false;
                if let Some(JSONSchemaPropsOrBool::Schema(s)) = &value.additional_properties {
                    let dict_type = s.type_.clone().unwrap_or_default();
                    if dict_type == "array" {
                        // unpack the inner object from the array wrap
                        if let Some(JSONSchemaPropsOrArray::Schema(items)) = &s.as_ref().items {
                            analyze_(*items.clone(), &next_key, &next_stack, level + 1, results)?;
                            handled_inner = true;
                        }
                    }
                    // TODO: not sure if these nested recurses are necessary - cluster test case does not have enough data
                    //if let Some(extra_props) = &s.properties {
                    //    for (_key, value) in extra_props {
                    //        debug!("nested recurse into {} {} - key: {}", next_key, next_stack, _key);
                    //        analyze_(value.clone(), &next_key, &next_stack, level +1, results)?;
                    //    }
                    //}
                }
                if !handled_inner {
                    // normal object recurse
                    analyze_(value, &next_key, &next_stack, level + 1, results)?;
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
                    analyze_(inner, &next_key, &next_stack, level + 1, results)?;
                }
            }
            "" => {
                if value.x_kubernetes_int_or_string.is_some() {
                    debug!("not recursing into IntOrString {}", key)
                } else {
                    debug!("not recursing into unknown empty type {}", key)
                }
            }
            x => {
                if let Some(en) = value.enum_ {
                    // plain enums do not need to recurse, can collect it here
                    let new_result = analyze_enum_properties(&en, &next_stack, level, &schema)?;
                    results.push(new_result);
                } else {
                    debug!("not recursing into {} ('{}' is not a container)", key, x)
                }
            }
        }
    }
    Ok(())
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
        //debug!("got enum {:?}", en);
        // TODO: do we need to verify enum elements? only in oneOf only right?
        let (name, rust_type) = match &en.0 {
            serde_json::Value::String(name) => (name, "".to_string()),
            _ => bail!("not handling non-string enum"),
        };
        // Create member and wrap types correctly
        let member_doc = None;
        debug!("with enum member {} of type {}", name, rust_type);
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


// helper to figure out what output structs (returned) and embedded members are contained in the current object schema
fn analyze_object_properties(
    props: &BTreeMap<String, JSONSchemaProps>,
    stack: &str,
    array_recurse_level: &mut HashMap<String, u8>,
    level: u8,
    schema: &JSONSchemaProps,
) -> Result<Vec<Container>, anyhow::Error> {
    let mut results = vec![];
    let mut members = vec![];
    //debug!("analyzing object {}", serde_json::to_string(&schema).unwrap());
    debug!("analyze object props in {}", stack);
    let reqs = schema.required.clone().unwrap_or_default();
    for (key, value) in props {
        debug!("analyze key {}", key);
        let value_type = value.type_.clone().unwrap_or_default();
        let rust_type = match value_type.as_ref() {
            "object" => {
                let mut dict_key = None;
                if let Some(additional) = &value.additional_properties {
                    debug!("got additional: {}", serde_json::to_string(&additional)?);
                    if let JSONSchemaPropsOrBool::Schema(s) = additional {
                        // This case is for maps. It is generally String -> Something, depending on the type key:
                        let dict_type = s.type_.clone().unwrap_or_default();
                        dict_key = match dict_type.as_ref() {
                            "string" => Some("String".into()),
                            // We are not 100% sure the array and object subcases here are correct but they pass tests atm.
                            // authoratative, but more detailed sources than crd validation docs below are welcome
                            // https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#validation
                            "array" => {
                                let mut simple_inner = None;
                                if let Some(inner) = &s.items {
                                    if let JSONSchemaPropsOrArray::Schema(ix) = inner {
                                        simple_inner = ix.type_.clone();
                                        debug!("additional simple inner  type: {:?}", simple_inner);
                                    }
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
                                    Some("object") => {
                                        Some(format!("{}{}", stack, uppercase_first_letter(key)))
                                    }
                                    None => Some(format!("{}{}", stack, uppercase_first_letter(key))),

                                    // leftovers, array of arrays?... need a better way to recurse probably
                                    Some(x) => bail!("unknown inner empty dict type {} for {}", x, key),
                                }
                            }
                            "object" => {
                                // cluster test with `failureDomains` uses this spec format
                                Some(format!("{}{}", stack, uppercase_first_letter(key)))
                            }
                            "" => {
                                if s.x_kubernetes_int_or_string.is_some() {
                                    Some("IntOrString".into())
                                } else {
                                    bail!("unknown empty dict type for {}", key)
                                }
                            }
                            "integer" => Some(extract_integer_type(s)?),
                            // think the type we get is the value type
                            x => Some(uppercase_first_letter(x)), // best guess
                        };
                    }
                } else if value.properties.is_none()
                    && value.x_kubernetes_preserve_unknown_fields.unwrap_or(false)
                {
                    dict_key = Some("HashMap<String, serde_json::Value>".into());
                }
                if let Some(dict) = dict_key {
                    format!("BTreeMap<String, {}>", dict)
                } else {
                    format!("{}{}", stack, uppercase_first_letter(key))
                }
            }
            "string" => {
                if let Some(_en) = &value.enum_ {
                    debug!("got enum string: {}", serde_json::to_string(&schema).unwrap());
                    format!("{}{}", stack, uppercase_first_letter(key))
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
                    "IntOrString".into()
                } else if value.x_kubernetes_preserve_unknown_fields == Some(true) {
                    "HashMap<String, serde_json::Value>".into()
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
    results.push(Container {
        name: stack.to_string(),
        members,
        level,
        docs: schema.description.clone(),
        is_enum: false,
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
                if s.type_.is_none() && s.x_kubernetes_preserve_unknown_fields == Some(true) {
                    return Ok(("Vec<HashMap<String, serde_json::Value>>".into(), level));
                }
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

// unit tests particular schema patterns
#[cfg(test)]
mod test {
    use crate::analyze;
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::JSONSchemaProps;
    use serde_yaml;
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

        let structs = analyze(schema, "Agent").unwrap().0;
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
        let structs = analyze(schema, "Server").unwrap().0;
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
        assert_eq!(match_labels.type_, "HashMap<String, serde_json::Value>");
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

        let structs = analyze(schema, "Server").unwrap().0;
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
        let structs = analyze(schema, "MatchExpressions").unwrap().0;
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
        let structs = analyze(schema, "Endpoint").unwrap().0;
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
        let structs = analyze(schema, "ServerSpec").unwrap().0;
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
        let structs = analyze(schema, "ServiceMonitor").unwrap().0;
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
        let structs = analyze(schema, "DestinationRule").unwrap().0;
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
        let structs = analyze(schema, "KustomizationSpec").unwrap().0;
        println!("got {:?}", structs);
        let root = &structs[0];
        assert_eq!(root.name, "KustomizationSpec");
        assert_eq!(root.level, 0);
        assert_eq!(root.is_enum, false);
        assert_eq!(&root.members[0].name, "patchesStrategicMerge");
        assert_eq!(
            &root.members[0].type_,
            "Option<Vec<HashMap<String, serde_json::Value>>>"
        );
    }
}
