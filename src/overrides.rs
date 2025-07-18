use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    JSONSchemaProps, JSONSchemaPropsOrArray, JSONSchemaPropsOrBool, JSON,
};
use regex::RegexSet;
use serde::{Deserialize, Deserializer};
use std::{
    collections::{hash_map, BTreeMap, HashMap, HashSet},
    fs::File,
    iter,
    ops::Deref,
    path::Path,
    rc::Rc,
};

/// Configuration for overriding type-related aspects of code analysis and generation.
///
/// The serialized YAML format is:
///
/// ```yaml
/// propertyRules:
///     # The action to perform if the type and name-directed matches below succeed.
///   - matchSuccess:
///     # String to use as the property's Rust type. This will prevent a Rust `Container` being
///     # generated for the property, if matching succeeds.
///       replace: MyType
///     # Instead of replacing the property with an existing type, it can also be ignored using:
///     # matchSuccess: omit
///
///     # Zero or more match expressions to evaluate the property's name (key/member/field) against.
///     # Only _one_ of these expressions needs to match, for the rules engine to move on to
///     # evaluating the schema.
///     matchName:
///       - exact: foo
///       - regex: ^bar[1-9]+$
///
///     # A subset of the serialized `JSONSchemaProps` format to match against the property's schema.
///     #
///     # The specific subset of `JSONSchemaProps` fields are those that directly affect code
///     # generation. Much like `matchName`, there are two types of type-directed structural matches
///     # that can be performed,
///     # `subset` and `exhaustive`.
///     matchSchema:
///       # `subset` ensures that the property schema has _at least_ these fields.
///       # Additional fields in the property are permitted.
///       subset:
///         type: object
///         properties:
///           claims:
///             type: array
///           limits:
///             type: object
///           requests:
///             type: object
///
///       # Whereas `exhaustive` ensures the property schema matches _exactly_ these fields.
///       # Additional fields in the property are not permitted.
///       exhaustive:
///         type: ...
///         ...
/// ```
#[derive(Debug, Default)]
pub struct Overrides {
    /// An index of exact property names that should be matched to determine if type replacement
    /// should occur. This is checked prior to `property_rules` and exists as an optimization.
    property_index: HashMap<String, Vec<Rc<CompiledPropertyRule>>>,

    /// A sequence of rules that will evaluated in order to determine if type replacement should occur.
    property_rules: Vec<Rc<CompiledPropertyRule>>,
}

impl<'de> Deserialize<'de> for Overrides {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // The serialized representation for `Overrides`.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Overrides {
            #[serde(default, with = "serde_yaml::with::singleton_map_recursive")]
            property_rules: Vec<PropertyRule>,
        }

        Self::new(Overrides::deserialize(deserializer)?.property_rules).map_err(|errors| {
            let rendered = iter::once("Failed to compile regular expressions with:".to_owned())
                .chain(errors.into_iter().map(|error| error.to_string()))
                .collect::<Vec<_>>()
                .join("\n");

            serde::de::Error::custom(rendered)
        })
    }
}

impl Overrides {
    /// Create an overrides configuration from a iterator of uncompiled property rules.
    ///
    /// All regular expressions will be compiled, even if an error is encountered.
    /// Any `Err` result will contain one or more errors.
    pub fn new(property_rules: impl IntoIterator<Item = PropertyRule>) -> Result<Self, Vec<regex::Error>> {
        // Build the exact match property index and linear scan property rules in a single pass,
        // collecting any regex errors to return all failures to the caller.
        let mut errors = Vec::new();
        let mut property_index = HashMap::new();
        let property_rules = property_rules
            .into_iter()
            .filter_map(|rule| {
                let (rule, exact_matches) = rule
                    .compile()
                    .map_err(|error| {
                        errors.push(error);
                    })
                    .ok()?;
                // For each `name` in exact matches, a clone of the rule is stored in the
                // `property_index` `HashMap`, hence all compiled rules are wrapped in `Rc`.
                let rule = Rc::new(rule);
                let has_exact_matches = !exact_matches.is_empty();
                for name in exact_matches {
                    property_index
                        .entry(name)
                        .and_modify(|rules: &mut Vec<Rc<CompiledPropertyRule>>| rules.push(Rc::clone(&rule)))
                        .or_insert(vec![Rc::clone(&rule); 1]);
                }

                // Don't yield the rule for the linear scan if it has an exact match and _no_ regex matches,
                // since it's already been added to the index above.
                if has_exact_matches && rule.match_name.is_empty() {
                    None
                } else {
                    Some(rule)
                }
            })
            .collect();

        // If any regex errors were encountered, consider the initialization a failure.
        if errors.is_empty() {
            Ok(Self {
                property_index,
                property_rules,
            })
        } else {
            Err(errors)
        }
    }

    /// Load multiple sets of overrides from disparate files into a single set of overrides.
    ///
    /// If override configuration exists for a particular property, any existing [`PropertyRule`]s
    /// are preserved and the additional rules are appended.
    pub fn from_paths<P: AsRef<Path>>(mut paths: impl Iterator<Item = P>) -> anyhow::Result<Self> {
        paths.try_fold(Self::default(), |mut overrides, path| {
            overrides.extend([serde_yaml::from_reader(File::open(path)?)?]);
            Ok(overrides)
        })
    }

    /// Get the first configured rule that matches the supplied property name and value.
    pub fn get_property_action(&self, name: &str, schema: &JSONSchemaProps) -> Option<&PropertyAction> {
        self.get_property_rule(name, schema)
            .map(|rule| &rule.match_success)
    }

    /// Get the first configured rule that matches the supplied property name and value.
    ///
    /// If rules are found that exactly match `name`, they will be tested in-order until either
    /// a rule matches, in which case the operation short-circuits and the rule is returned.
    ///
    /// If no rules are found that exactly match `name`, the full set of rules will be scanned,
    /// with the same short-circuiting behavior as above.
    ///
    /// If no rules are found that match, or the set of rules are exhausted, [`None`] is returned.
    fn get_property_rule(&self, name: &str, schema: &JSONSchemaProps) -> Option<&CompiledPropertyRule> {
        // Check the index for an exact match.
        if let Some(rules) = self.property_index.get(name) {
            for rule in rules {
                if rule.is_match(name, schema) {
                    return Some(rule);
                }
            }
        }

        // Otherwise, perform a sequential scan.
        self.property_rules
            .iter()
            .find(|rule| rule.is_match(name, schema))
            .map(|rule| &**rule)
    }
}

/// Naively union multiple [`Overrides`] by concatenating rules.
///
/// Because order is important when applying rules, we cannot guarantee the merged overrides
/// are fully deduplicated due to lack of sorting.
impl Extend<Self> for Overrides {
    fn extend<T: IntoIterator<Item = Self>>(&mut self, iter: T) {
        for Self {
            property_rules,
            property_index,
        } in iter
        {
            // Extend the index by merging rules for existing keys.
            for (name, rules) in property_index {
                match self.property_index.entry(name) {
                    hash_map::Entry::Occupied(mut old) => {
                        old.get_mut().extend(rules);
                    }
                    hash_map::Entry::Vacant(new) => {
                        new.insert(rules);
                    }
                }
            }

            // Extend the linear scan by concatenating rules and then naively deduping based on
            // `PartialEq`, which is all `JSONSchemaProps` provides.
            self.property_rules.extend(property_rules);

            // We perform this repeatedly after each extension, in an attempt to minimize potential
            // spare duplicates due to the lack of sorting.
            self.property_rules.dedup();
        }
    }
}

/// A rule applicable to the key/value pairs in [`JSONSchemaProps::properties`].
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyRule<N = PropertyNameSet> {
    /// A set of expressions that will be evaluated against a property's name.
    ///
    /// Only _one_ of the expressions needs to match, for the entired name-directed match to succeed,
    /// at which point the type-directed schema match will be evaluated.
    ///
    /// If absent, only type-directed matches are required for this rule to succeed.
    #[serde(default)]
    pub match_name: N,

    /// The schema expression that a CRD property's schema will be evaluated against.
    ///
    /// If absent, only name-directed matches are required for this rule to succeed.
    pub match_schema: Option<PropertySchema>,

    /// The behavior of this rule if the type and name-directed match phases succeed.
    pub match_success: PropertyAction,
}

impl PropertyRule {
    /// Compile any regular expressions contained in the property rule, returning a set of exact
    /// matches that were not compiled into the resulting regular expression set, so they can be
    /// optimized elsewhere.
    fn compile(self) -> Result<(CompiledPropertyRule, HashSet<String>), regex::Error> {
        let mut exact_matches = HashSet::new();
        let regex_matches = self.match_name.into_iter().filter_map(|name| match name {
            PropertyName::Regex(regex) => Some(regex),
            PropertyName::Exact(exact) => {
                exact_matches.insert(exact);
                None
            }
        });

        Ok((
            PropertyRule {
                match_name: PropertyRegexSet::new(regex_matches)?,
                match_schema: self.match_schema,
                match_success: self.match_success,
            },
            exact_matches,
        ))
    }
}


/// The compiled representation used for matching rules during a linear scan.
type CompiledPropertyRule = PropertyRule<PropertyRegexSet>;

impl CompiledPropertyRule {
    /// Determine if this rule matches the supplied `name` _and_ `schema`.
    fn is_match(&self, name: &str, schema: &JSONSchemaProps) -> bool {
        if !self.match_name.is_empty() && !self.match_name.is_match(name) {
            return false;
        }

        if let Some(match_schema) = &self.match_schema {
            if match_schema != schema {
                return false;
            }
        }

        true
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyAction {
    /// The type name that should be used verbatim as a replacement, instead of generating any
    /// nested container, if the associated rule matches.
    Replace(String),

    /// If the property should be ignored and omitted entirely from any containers, if the
    /// associated rule matches.
    Omit,
}

/// A [`PartialEq`] wrapper around [`RegexSet`], see <https://github.com/rust-lang/regex/issues/364>.
#[derive(Debug)]
struct PropertyRegexSet(RegexSet);

impl PropertyRegexSet {
    fn new<S>(patterns: impl IntoIterator<Item = S>) -> Result<Self, regex::Error>
    where
        S: AsRef<str>,
    {
        RegexSet::new(patterns).map(Self)
    }
}

impl Deref for PropertyRegexSet {
    type Target = RegexSet;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq for PropertyRegexSet {
    fn eq(&self, other: &Self) -> bool {
        self.patterns() == other.patterns()
    }
}

/// The serialized config representation for `matchName`.
pub type PropertyNameSet = HashSet<PropertyName>;

/// An expression defining how a name match should be performed.
#[derive(Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyName {
    /// Should the property name match this string exactly?
    Exact(String),

    /// Should the property name match this regular expression?
    Regex(String),
}

/// An expression defining how a schema match should be performed.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertySchema {
    /// Should the property schema match this exact set of fields?
    Exhaustive(JSONSchemaProps),

    /// Should the property schema contain a superset of these fields?
    Subset(JSONSchemaProps),
}

/// Equality for a [`PropertySchema`] expression considers only a subset of fields for comparison.
impl PartialEq<JSONSchemaProps> for PropertySchema {
    fn eq(&self, other: &JSONSchemaProps) -> bool {
        match self {
            Self::Exhaustive(x) => x.is_exhaustive(other),
            Self::Subset(x) => x.is_subset(other),
        }
    }
}

/// Refined [`PartialEq`]-like behavior for comparing [`JSONSchemaProps`].
///
/// Using a new trait here allows changing the existing [`PartialEq`] semantics of [`JSONSchemaProps`]
/// to only consider a subset of fields that potentially affect code generation.
///
// Note: all of the implementations below could be `#[inline]`d.
trait SchemaEq {
    fn is_exhaustive(&self, other: &Self) -> bool;
    fn is_subset(&self, other: &Self) -> bool;
}

impl SchemaEq for JSONSchemaProps {
    fn is_exhaustive(&self, other: &Self) -> bool {
        macro_rules! exhaustive {
           ( $($field:ident),* $(,)? ) => {
               $(
                   if !self.$field.is_exhaustive(&other.$field) {
                       return false;
                   }
               )*
           }
        }

        // The subset of fields are limited to those that can potentially affect code generation.
        exhaustive! {
            type_,
            enum_,
            items,
            additional_items,
            properties,
            additional_properties,
            required,
            one_of,
            all_of,
            any_of,
            not,
            x_kubernetes_int_or_string,
            x_kubernetes_preserve_unknown_fields,
            x_kubernetes_list_type,
            x_kubernetes_map_type,
        }

        true
    }

    fn is_subset(&self, other: &Self) -> bool {
        macro_rules! subset {
           ( $($field:ident),* $(,)? ) => {
               $(
                   if !self.$field.is_subset(&other.$field) {
                       return false;
                   }
               )*
           }
        }

        // The subset of fields are limited to those that can potentially affect code generation.
        subset! {
            type_,
            enum_,
            items,
            additional_items,
            properties,
            additional_properties,
            required,
            one_of,
            all_of,
            any_of,
            not,
            x_kubernetes_int_or_string,
            x_kubernetes_preserve_unknown_fields,
            x_kubernetes_list_type,
            x_kubernetes_map_type,
        }

        true
    }
}

impl SchemaEq for JSONSchemaPropsOrArray {
    fn is_exhaustive(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Schemas(xs), Self::Schemas(ys)) => xs.is_exhaustive(ys),
            (Self::Schemas(_), Self::Schema(_)) => false,
            (Self::Schema(_), Self::Schemas(_)) => false,
            (Self::Schema(x), Self::Schema(y)) => x.is_exhaustive(y),
        }
    }

    fn is_subset(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Schemas(xs), Self::Schemas(ys)) => xs.is_subset(ys),
            (Self::Schemas(xs), Self::Schema(y)) => xs.iter().all(|x| x.is_subset(y)),
            (Self::Schema(x), Self::Schemas(ys)) => ys.iter().any(|y| x.as_ref().is_subset(y)),
            (Self::Schema(x), Self::Schema(y)) => x.is_subset(y),
        }
    }
}

impl SchemaEq for JSONSchemaPropsOrBool {
    fn is_exhaustive(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Schema(x), Self::Schema(y)) => x.is_exhaustive(y),
            (Self::Schema(_), Self::Bool(_)) => false,
            (Self::Bool(x), Self::Bool(y)) => x == y,
            (Self::Bool(_), Self::Schema(_)) => false,
        }
    }

    fn is_subset(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Schema(x), Self::Schema(y)) => x.is_subset(y),
            (Self::Schema(_), Self::Bool(_)) => false,
            (Self::Bool(x), Self::Bool(y)) => x == y,
            (Self::Bool(_), Self::Schema(_)) => false,
        }
    }
}

impl SchemaEq for JSON {
    fn is_exhaustive(&self, other: &Self) -> bool {
        self.0.is_exhaustive(&other.0)
    }

    fn is_subset(&self, other: &Self) -> bool {
        self.0.is_subset(&other.0)
    }
}

impl SchemaEq for serde_json::Value {
    fn is_exhaustive(&self, other: &Self) -> bool {
        self.eq(other)
    }

    fn is_subset(&self, other: &Self) -> bool {
        use serde_json::Value::*;
        match (self, other) {
            (Object(xs), Object(ys)) => xs.is_subset(ys),
            (Array(xs), Array(ys)) => xs.is_subset(ys),
            (String(x), String(y)) => x == y,
            (Number(x), Number(y)) => x == y,
            (Bool(x), Bool(y)) => x == y,
            (Null, _) => true,
            (_, _) => false,
        }
    }
}

impl<T: ?Sized + SchemaEq> SchemaEq for &T {
    fn is_exhaustive(&self, other: &&T) -> bool {
        SchemaEq::is_exhaustive(*self, *other)
    }

    fn is_subset(&self, other: &Self) -> bool {
        SchemaEq::is_subset(*self, *other)
    }
}

impl<T: SchemaEq> SchemaEq for Box<T> {
    fn is_exhaustive(&self, other: &Self) -> bool {
        SchemaEq::is_exhaustive(&**self, &**other)
    }

    fn is_subset(&self, other: &Self) -> bool {
        SchemaEq::is_subset(&**self, &**other)
    }
}

impl<T: SchemaEq> SchemaEq for Option<T> {
    fn is_exhaustive(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(x), Some(y)) => x.is_exhaustive(y),
            (Some(_), None) => false,
            (None, Some(_)) => false,
            (None, None) => true,
        }
    }

    fn is_subset(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(x), Some(y)) => x.is_subset(y),
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => true,
        }
    }
}

impl SchemaEq for serde_json::Map<String, serde_json::Value> {
    fn is_exhaustive(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        for ((k1, v1), (k2, v2)) in self.iter().zip(other.iter()) {
            if k1 != k2 || !v1.is_exhaustive(v2) {
                return false;
            }
        }

        true
    }

    fn is_subset(&self, other: &Self) -> bool {
        if self.len() > other.len() {
            return false;
        }

        for (k, x) in self {
            match other.get(k) {
                Some(y) if x.is_subset(y) => continue,
                _ => return false,
            }
        }

        true
    }
}

impl<V: SchemaEq> SchemaEq for BTreeMap<String, V> {
    fn is_exhaustive(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        for ((k1, v1), (k2, v2)) in self.iter().zip(other.iter()) {
            if k1 != k2 || !v1.is_exhaustive(v2) {
                return false;
            }
        }

        true
    }

    fn is_subset(&self, other: &Self) -> bool {
        if self.len() > other.len() {
            return false;
        }

        for (k, x) in self {
            match other.get(k) {
                Some(y) if x.is_subset(y) => continue,
                _ => return false,
            }
        }

        true
    }
}

impl<T: SchemaEq> SchemaEq for Vec<T> {
    fn is_exhaustive(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        for (x, y) in self.iter().zip(other.iter()) {
            if !x.is_exhaustive(y) {
                return false;
            }
        }

        true
    }

    fn is_subset(&self, other: &Self) -> bool {
        if self.len() > other.len() {
            return false;
        }

        for x in self {
            if !other.iter().any(|y| x.is_subset(y)) {
                return false;
            }
        }

        true
    }
}

macro_rules! impl_primitive_eq {
    ( $($type:ty),* $(,)? ) => {
        $(
            impl SchemaEq for $type {
                #[inline]
                fn is_exhaustive(&self, other: &Self) -> bool {
                    self.eq(other)
                }

                #[inline]
                fn is_subset(&self, other: &Self) -> bool {
                    self.eq(other)
                }
            }
        )*
    };
}

impl_primitive_eq! {
    bool,
    i64,
    f64,
    str,
    String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_btree_map() {
        let subset = BTreeMap::from_iter([("foo".to_owned(), true), ("baz".to_owned(), true)]);
        let superset = BTreeMap::from_iter([
            ("foo".to_owned(), true),
            ("bar".to_owned(), true),
            ("baz".to_owned(), true),
        ]);

        assert!(superset.is_exhaustive(&superset), "identity should be exhaustive");
        assert!(
            superset.is_subset(&superset),
            "identity should be a non-proper subset"
        );

        assert!(
            !subset.is_exhaustive(&superset),
            "subset should _not_ be exhaustive"
        );
        assert!(
            subset.is_subset(&superset),
            "subset should be a non-proper subset"
        );

        assert!(
            !superset.is_exhaustive(&subset),
            "superset should _not_ be exhaustive"
        );
        assert!(
            !superset.is_subset(&subset),
            "subset should _not_ be a non-proper subset"
        );
    }

    #[test]
    fn test_local_object_reference() {
        let subset = serde_yaml::from_str::<JSONSchemaProps>(
            r#"
            type: array
            items:
              type: object
              properties:
                name:
                  type: string
            "#,
        )
        .unwrap();
        let superset = serde_yaml::from_str::<JSONSchemaProps>(
            r#"
            type: array
            minItems: 1
            items:
              type: object
              properties:
                name:
                  type: string
                  pattern: "^[a-z0-9]{1,11}$"
                  description: "Name of the listener."
                port:
                  type: integer
                  minimum: 9092
                  description: "Port number used by the listener."
                type:
                  type: string
                  enum:
                  - internal
                  - cluster-ip
            "#,
        )
        .unwrap();

        assert!(
            !subset.is_exhaustive(&superset),
            "expected exhaustive match failure, got success"
        );
        assert!(
            subset.is_subset(&superset),
            "expected subset match success, got failure"
        );
    }
}
