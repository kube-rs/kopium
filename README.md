# kopium

[![CI](https://github.com/kube-rs/kopium/actions/workflows/release.yml/badge.svg)](https://github.com/kube-rs/kopium/actions/workflows/release.yml)
[![Crates.io](https://img.shields.io/crates/v/kopium.svg)](https://crates.io/crates/kopium)

**K**ubernetes **op**enap**i** **u**n**m**angler.

Generates rust structs from `customresourcedefinitions` in your kubernetes cluster follwing the spec/status model, by using their embedded openapi schema.

**⚠️ WARNING: [not feature complete](https://github.com/kube-rs/kopium/issues) ⚠️**

Requirements:

- [stable](https://kubernetes.io/blog/2019/09/18/kubernetes-1-16-release-announcement/#custom-resources-reach-general-availability) `customresourcedefinition` with schema
- crd following standard [spec/status model](https://kubernetes.io/docs/concepts/overview/working-with-objects/kubernetes-objects/#object-spec-and-status)

## Features

- **Instantly queryable**: generated type uses [`kube-derive`](https://docs.rs/kube/latest/kube/derive.CustomResource.html) to provide api integration with `kube`
- **[Rust doc comments](https://doc.rust-lang.org/rust-by-example/meta/doc.html#doc-comments)**: optionally extracted from `description` values in schema
- **Safe case [conversion](https://github.com/withoutboats/heck)**: generated types uses rust standard camel_case with occasional [serde rename attributes](https://serde.rs/field-attrs.html)
- **Usable locally and in CI**: Can read crds by name in cluster via `mycrd.group.io` or from file via `-f crd.yaml`

## Installation

Grab a prebuilt musl/darwin binary from the [latest release](https://github.com/kube-rs/kopium/releases), or install from [crates.io](https://crates.io/crates/kopium):

```sh
cargo install kopium
```

## Usage

If you have a crd installed in your cluster, pass a crd name accessible from your `KUBECONFIG`:

```sh
kopium prometheusrules.monitoring.coreos.com -A > prometheusrule.rs
```

Or pass a file/stdin via `-f`:

```sh
curl -sSL https://raw.githubusercontent.com/prometheus-operator/prometheus-operator/main/example/prometheus-operator-crd/monitoring.coreos.com_prometheusrules.yaml \
    | kopium -f - -A > prometheusrule.rs
```

## Output

```rust
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

/// Specification of desired alerting rule definitions for Prometheus.
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(group = "monitoring.coreos.com", version = "v1", kind = "PrometheusRule", plural = "prometheusrules")]
#[kube(namespaced)]
pub struct PrometheusRuleSpec {
    /// Content of Prometheus rule file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<PrometheusRuleGroups>>,
}

/// RuleGroup is a list of sequentially evaluated recording and alerting rules. Note: PartialResponseStrategy is only used by ThanosRuler and will be ignored by Prometheus instances.  Valid values for this field are 'warn' or 'abort'.  More info: https://github.com/thanos-io/thanos/blob/main/docs/components/rule.md#partial-response
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct PrometheusRuleGroups {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_response_strategy: Option<String>,
    pub rules: Vec<PrometheusRuleGroupsRules>,
}

/// Rule describes an alerting or recording rule See Prometheus documentation: [alerting](https://www.prometheus.io/docs/prometheus/latest/configuration/alerting_rules/) or [recording](https://www.prometheus.io/docs/prometheus/latest/configuration/recording_rules/#recording-rules) rule
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct PrometheusRuleGroupsRules {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    pub expr: IntOrString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
}
```

## Usage with kube

Simply add the generated file (e.g. output from above in `prometheusrule.rs`) to your library, and import (at least) the special root type:


```rust
use prometheusrule::PrometheusRule;
use kube::{Api, Client, ResourceExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::try_default().await?;
    let pr: Api<PrometheusRule> = Api::default_namespaced(client);
    for p in pr.list(&Default::default()).await? {
        println!("Found PrometheusRule {} in current namespace", p.name());
    }
    Ok(())
}
```

## Autocomplete

Autocompletion for most shells available via `kopium completions`:

```sh
source <(kopium completions bash)
```

## Testing

Unit tests and running `kopium` from a file do not require a cluster and can be run with:

```sh
cargo test --lib
cargo run --bin kopium -- -f mycrd.yaml -A
```

Full integration tests use your current cluster to try to read a CRD and a `gen` object (instance of the CRD type) and parse it into the generated type:

```sh
cargo run --bin kopium -- -f prometheusrules.monitoring.coreos.com > tests/gen.rs
echo "pub type CR = PrometheusRule;" >> tests/gen.rs
kubectl apply -f tests/pr.yaml # needs to contain a CR with name "gen"
cargo test --test runner -- --nocapture
```

test shortcuts available via `just` in the [`justfile`](./justfile) and run pre-merge.

## License

Apache 2.0 licensed. See LICENSE for details.
