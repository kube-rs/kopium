default:
  @just --list --unsorted

fmt:
  cargo +nightly fmt
  cd examples && cargo +nightly fmt
  rustfmt +nightly tests/generated/*.rs

lint:
  cargo clippy

test: download-crds gen-tests test-runner test-trycmd-verify

generate-runner-binding crd_path resource out_file extra_args='':
  RUNNER_GEN_CRD_PATH={{crd_path}} RUNNER_GEN_RESOURCE={{resource}} RUNNER_GEN_OUT_DIR=tests/generated RUNNER_GEN_OUT_FILE={{out_file}} RUNNER_GEN_EXTRA_ARGS="{{extra_args}}" cargo test --test generate_runner_bindings -- --ignored --nocapture

download-crds: download-crd-prom download-crd-argo download-crd-certmanager
  mkdir -p tests/generated

download-crd-prom:
  #!/usr/bin/env bash
  version="0.89.0"
  curl -sSL https://github.com/prometheus-operator/prometheus-operator/releases/download/v${version}/stripped-down-crds.yaml \
  | lq . -y --split '"tests/" + (.metadata.name) + ".yaml"'
  rm tests/{alertmanager*,probes,prometheusagents,prometheuses,scrapeconfigs,thanosrulers}.monitoring.coreos.com.yaml

download-crd-argo:
  curl -sSL https://raw.githubusercontent.com/argoproj/argo-cd/master/manifests/crds/application-crd.yaml -o tests/generated/application-crd.yaml

download-crd-certmanager:
  curl -sSL https://github.com/jetstack/cert-manager/releases/download/v1.7.1/cert-manager.crds.yaml -o tests/generated/cert-manager.crds.yaml

_gen file +ARGS:
  cargo run --bin kopium -- {{ARGS}} > tests/generated/{{file}}

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

test-runner:
  cargo test --test runner

test-trycmd:
  TRYCMD=overwrite cargo test --test trycmd_tests

test-trycmd-verify:
  cargo test --test trycmd_tests

examples:
  cd examples && cargo build

release:
  cargo release minor --execute
