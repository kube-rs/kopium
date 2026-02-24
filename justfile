default:
  @just --list --unsorted

fmt:
  cargo +nightly fmt
  cd examples && cargo +nightly fmt
  rustfmt +nightly tests/generated/*.rs

lint:
  cargo clippy

examples:
  cd examples && cargo build

[group('test'), doc('run all tests, and fully generate tests folder for integration tests')]
test: download-crds gen-tests test-runner test-trycmd-verify

[group('test'), doc('run integration tests with current tests/generated folder')]
test-runner:
  cargo test --test runner

[group('test'), doc("run trycmd tests with OVERWRITE")]
test-trycmd:
  TRYCMD=overwrite cargo test --test trycmd_tests

[group('test'), doc("run trycmd tests in VERIFY")]
test-trycmd-verify:
  cargo test --test trycmd_tests

[private]
_gen file +ARGS:
  cargo run --bin kopium -- {{ARGS}} > tests/generated/{{file}}

[group('test'), doc('generate rust files from crds via kopium')]
gen-tests:
  just _gen prometheusrule.rs -f tests/prometheusrules.monitoring.coreos.com.yaml
  just _gen podmonitor.rs -f tests/podmonitors.monitoring.coreos.com.yaml
  just _gen servicemonitor.rs -df tests/servicemonitors.monitoring.coreos.com.yaml
  just _gen multiversion.rs -Af tests/mv-crd.yaml
  just _gen agent.rs -bAf tests/agent-crd.yaml
  # just _gen application.rs applications.argoproj.io
  # ! just _gen unused.rs -f tests/argoproj.io_clusterworkflowtemplates.yaml
  # ! just _gen unused2.rs --relaxed --filename tests/argoproj.io_clusterworkflowtemplates.yaml
  # just _gen certificate.rs -d certificates.cert-manager.io
  just _gen cluster.rs  -f tests/cluster-crd.yaml -d
  just _gen httproute.rs -f tests/httproute-crd.yaml
  # just _gen serverauthorization.rs -d serverauthorizations.policy.linkerd.io
  #just _gen policy.rs -b servers.policy.linkerd.io
  # just _gen destinationrule.rs destinationrules.networking.istio.io

[group('test'), doc('download all crds for integration test runner')]
download-crds: && download-crd-prom download-crd-argo download-crd-certmanager
  mkdir -p tests/generated

[group('test')]
download-crd-prom:
  #!/usr/bin/env bash
  version="0.89.0"
  curl -sSL https://github.com/prometheus-operator/prometheus-operator/releases/download/v${version}/stripped-down-crds.yaml \
  | lq . -y --split '"tests/" + (.metadata.name) + ".yaml"'
  rm tests/{alertmanager*,probes,prometheusagents,prometheuses,scrapeconfigs,thanosrulers}.monitoring.coreos.com.yaml

[group('test')]
download-crd-argo:
  curl -sSL https://raw.githubusercontent.com/argoproj/argo-cd/master/manifests/crds/application-crd.yaml -o tests/generated/application-crd.yaml

# inlining these instead atm
# [group('test')]
# download-crd-gateway:
#   #!/usr/bin/env/bash
#   version="1.4.1"
#   curl -sSL https://github.com/kubernetes-sigs/gateway-api/releases/download/v${version}/standard-install.yaml > tests/generated/gateway-crds.yaml
# [group('test')]
# download-linkerd-crds:
#   helm template linkerd-edge/linkerd-crds --version 2025.10.7 > tests/generated/linkerd-crds.yaml

[group('test')]
download-crd-certmanager:
  #!/usr/bin/env bash
  version="1.19.1"
  echo curl -sSL https://github.com/cert-manager/cert-manager/releases/download/v${version}/cert-manager.crds.yaml -o tests/generated/cert-manager.crds.yaml

[group('maintainer')]
release:
  cargo release minor --execute
