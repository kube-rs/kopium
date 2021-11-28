# kopium

[![CI](https://github.com/kube-rs/kopium/actions/workflows/release.yml/badge.svg)](https://github.com/kube-rs/kopium/actions/workflows/release.yml)
[![Crates.io](https://img.shields.io/crates/v/kopium.svg)](https://crates.io/crates/kopium)

**K**ubernetes **op**enap**i** **u**n**m**angler.

Generates rust structs from `customresourcedefinitions` in your kubernetes cluster follwing the spec/status model, by using their embedded openapi schema.

**⚠️ WARNING: [ALPHA SOFTWARE](https://github.com/kube-rs/kopium/issues) ⚠️**

Requirements:

- stable `customresourcedefinition` ([v1beta1 was removed in v1.22](https://kubernetes.io/blog/2021/07/14/upcoming-changes-in-kubernetes-1-22/))
- crd with spec/status

## Installation

Grab a prebuilt musl/darwin binary from the [latest release](https://github.com/kube-rs/kopium/releases), or install from [crates.io](https://crates.io/crates/kopium):

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
    pub groups: Vec<PrometheusRuleSpecGroups>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrometheusRuleSpecGroups {
    pub interval: Option<String>,
    pub name: String,
    pub partial_response_strategy: Option<String>,
    pub rules: Vec<PrometheusRuleSpecGroupsRules>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrometheusRuleSpecGroupsRules {
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

Requires kubernetes access to write customresourcedefinitions.
