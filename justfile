NAME := "kopium"
REPO := "kube-rs"
VERSION := `git rev-parse HEAD`
SEMVER_VERSION := `grep version Cargo.toml | awk -F"\"" '{print $2}' | head -n 1`

default:
  @just --list --unsorted | grep -v "    default"

test: test-pr test-mv test-argo

test-pr:
  kubectl apply --force-conflicts --server-side -f https://raw.githubusercontent.com/prometheus-operator/prometheus-operator/v0.52.0/example/prometheus-operator-crd/monitoring.coreos.com_prometheusrules.yaml
  cargo run --bin kopium -- prometheusrules.monitoring.coreos.com > tests/gen.rs
  echo "pub type CR = PrometheusRule;" >> tests/gen.rs
  kubectl apply -f tests/pr.yaml
  cargo test --test runner -- --nocapture

test-mv:
  kubectl apply -f tests/mv-crd.yaml
  cargo run --bin kopium -- multiversions.clux.dev > tests/gen.rs
  echo "pub type CR = MultiVersion;" >> tests/gen.rs
  kubectl apply -f tests/mv.yaml
  cargo test --test runner -- --nocapture

test-agentshift:
  kubectl apply -f tests/agentshift-crd.yaml
  cargo run --bin kopium -- agents.agent-install.openshift.io > tests/gen.rs
  echo "pub type CR = Agent;" >> tests/gen.rs
  cargo build --test runner

test-argo:
  kubectl apply --force-conflicts --server-side -f https://raw.githubusercontent.com/argoproj/argo-cd/master/manifests/crds/application-crd.yaml
  cargo run --bin kopium -- applications.argoproj.io > tests/gen.rs
  echo "pub type CR = Application;" >> tests/gen.rs
  kubectl apply -f tests/app.yaml
  cargo test --test runner -- --nocapture

release:
  cargo release minor --execute
