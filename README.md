# kopium

[![CI](https://github.com/kube-rs/kopium/actions/workflows/release.yml/badge.svg)](https://github.com/kube-rs/kopium/actions/workflows/release.yml)
[![Crates.io](https://img.shields.io/crates/v/kopium.svg)](https://crates.io/crates/kopium)


A **k**ubernetes **op**enap**i** **u**n**m**angler.

Creates rust structs from a named crd by converting the live openapi schema.


**⚠️ WARNING: ALPHA SOFTWARE ⚠️**

## Installation

```sh
cargo install kopium
```

## Usage

```sh
kopium prometheusrules.monitoring.coreos.com > prometheusrule.rs
rustfmt +nightly --edition 2021 prometheusrule.rs
```

## Output

```rust
use kube::CustomResource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug)]
#[kube(
    group = "monitoring.coreos.com",
    version = "v1",
    kind = "PrometheusRule",
    plural = "prometheusrules"
)]
#[kube(namespaced)]
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

## Testing

Generate a CRD, tell the test runner to try to use it.

```sh
cargo run --bin kopium -- prometheusrules.monitoring.coreos.com > tests/gen.rs
echo "pub type CR = PrometheusRule;" >> tests/gen.rs
kubectl apply -f tests/pr.yaml # needs to contain a CR with name "gen"
cargo test --test runner -- --nocapture
```
