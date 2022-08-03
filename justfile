NAME := "kopium"
REPO := "kube-rs"
VERSION := `git rev-parse HEAD`
SEMVER_VERSION := `grep version Cargo.toml | awk -F"\"" '{print $2}' | head -n 1`

default:
  @just --list --unsorted | grep -v "    default"

fmt:
  cargo +nightly fmt

test: test-pr test-sm test-mv test-argo test-agent test-certmanager test-cluster test-linkerd-serverauth test-linkerd-server

test-pr:
  kubectl apply --force-conflicts --server-side -f https://raw.githubusercontent.com/prometheus-operator/prometheus-operator/v0.52.0/example/prometheus-operator-crd/monitoring.coreos.com_prometheusrules.yaml
  cargo run --bin kopium -- prometheusrules.monitoring.coreos.com > tests/gen.rs
  echo "pub type CR = PrometheusRule;" >> tests/gen.rs
  kubectl apply -f tests/pr.yaml
  cargo test --test runner -- --nocapture

test-sm:
  kubectl apply --force-conflicts --server-side -f tests/servicemon-crd.yaml
  cargo run --bin kopium -- -df tests/servicemon-crd.yaml > tests/gen.rs
  echo "pub type CR = ServiceMonitor;" >> tests/gen.rs
  kubectl apply -f tests/servicemon.yaml
  cargo test --test runner -- --nocapture

test-mv:
  kubectl apply -f tests/mv-crd.yaml
  cargo run --bin kopium -- multiversions.clux.dev -A > tests/gen.rs
  echo "pub type CR = MultiVersion;" >> tests/gen.rs
  kubectl apply -f tests/mv.yaml
  cargo test --test runner -- --nocapture

test-agent:
  kubectl apply -f tests/agent-crd.yaml
  cargo run --bin kopium -- -bAf tests/agent-crd.yaml > tests/gen.rs
  echo "pub type CR = Agent;" >> tests/gen.rs
  kubectl apply -f tests/agent.yaml
  cargo test --test runner -- --nocapture

test-argo:
  kubectl apply --force-conflicts --server-side -f https://raw.githubusercontent.com/argoproj/argo-cd/master/manifests/crds/application-crd.yaml
  cargo run --bin kopium -- applications.argoproj.io > tests/gen.rs
  echo "pub type CR = Application;" >> tests/gen.rs
  kubectl apply -f tests/app.yaml
  cargo test --test runner -- --nocapture

test-certmanager:
  kubectl apply --force-conflicts --server-side -f https://github.com/jetstack/cert-manager/releases/download/v1.7.1/cert-manager.crds.yaml
  cargo run --bin kopium -- -d certificates.cert-manager.io > tests/gen.rs
  echo "pub type CR = Certificate;" >> tests/gen.rs
  kubectl apply -f tests/cert.yaml
  cargo test --test runner -- --nocapture

test-cluster:
  kubectl apply -f tests/cluster-crd.yaml
  cargo run --bin kopium -- -f tests/cluster-crd.yaml -d > tests/gen.rs
  echo "pub type CR = Cluster;" >> tests/gen.rs
  # No test instance for this crd
  cargo build --test runner

test-linkerd-serverauth:
  kubectl apply --server-side -f tests/serverauth-crd.yaml
  cargo run --bin kopium -- -d serverauthorizations.policy.linkerd.io > tests/gen.rs
  echo "pub type CR = ServerAuthorization;" >> tests/gen.rs
  kubectl apply -f tests/serverauth.yaml
  cargo test --test runner -- --nocapture

test-linkerd-server:
  #kubectl apply --server-side -f tests/server-crd.yaml
  #cargo run --bin kopium -- -b servers.policy.linkerd.io > tests/gen.rs
  #echo "pub type CR = Server;" >> tests/gen.rs
  #kubectl apply -f tests/server.yaml
  #cargo test --test runner -- --nocapture

test-istio-destrule:
  kubectl apply --server-side -f tests/destinationrule-crd.yaml
  cargo run --bin kopium -- destinationrules.networking.istio.io > tests/gen.rs
  echo "pub type CR = DestinationRule;" >> tests/gen.rs
  kubectl apply -f tests/destinationrule.yaml
  # NB: this currently fails because of an empty status object with preserve-unknown-fields
  cargo test --test runner -- --nocapture

release:
  cargo release minor --execute
