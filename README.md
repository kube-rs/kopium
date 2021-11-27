# kopium

A **k**ubernetes **op**enap**i** **u**n**m**angler.

Creates rust structs from a named crd by converting the live openapi schema.


## Installation

```sh
cargo install kopium
```

## Usage

```sh
kopium prometheusrules.monitoring.coreos.com > prometheusrule.rs
```

## Output

```rust
use kube::CustomResource;
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug)]
#[kube(group = "monitoring.coreos.com", version = "v1", kind = "PrometheusRule")]
#[kube(Namespaced)]
#[kube(schema = "disabled")]
pub struct PrometheusRuleSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<PrometheusRuleGroups>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrometheusRuleGroups {
    pub interval: Option<String>,
    pub name: String,
    pub partial_response_strategy: Option<String>,
    pub rules: Vec<PrometheusRuleRules>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrometheusRuleRules {
    pub alert: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    pub expr: String,
    pub r#for: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub record: Option<String>,
}
```
