default:
  @just --list --unsorted

fmt:
  cargo +nightly fmt
  cd examples && cargo +nightly fmt
  rustfmt +nightly tests/generated/*.rs

lint:
  cargo clippy

test: generate-runner-bindings test-runner test-trycmd-verify

generate-runner-binding crd_path resource out_file extra_args='':
  RUNNER_GEN_CRD_PATH={{crd_path}} RUNNER_GEN_RESOURCE={{resource}} RUNNER_GEN_OUT_DIR=tests/generated RUNNER_GEN_OUT_FILE={{out_file}} RUNNER_GEN_EXTRA_ARGS="{{extra_args}}" cargo test --test generate_runner_bindings -- --ignored --nocapture

download-crd-pr:
  mkdir -p tests/generated
  curl -sSL https://raw.githubusercontent.com/prometheus-operator/prometheus-operator/v0.52.0/example/prometheus-operator-crd/monitoring.coreos.com_prometheusrules.yaml -o tests/generated/monitoring.coreos.com_prometheusrules.yaml

download-crd-argo:
  mkdir -p tests/generated
  curl -sSL https://raw.githubusercontent.com/argoproj/argo-cd/master/manifests/crds/application-crd.yaml -o tests/generated/application-crd.yaml

download-crd-certmanager:
  mkdir -p tests/generated
  curl -sSL https://github.com/jetstack/cert-manager/releases/download/v1.7.1/cert-manager.crds.yaml -o tests/generated/cert-manager.crds.yaml

generate-runner-bindings: generate-runner-bindings-parallel

[parallel]
generate-runner-bindings-parallel: \
  generate-runner-binding-prometheusrule \
  generate-runner-binding-servicemonitor \
  generate-runner-binding-multiversion \
  generate-runner-binding-agent \
  generate-runner-binding-application \
  generate-runner-binding-certificate \
  generate-runner-binding-cluster \
  generate-runner-binding-httproute \
  generate-runner-binding-serverauthorization \
  generate-runner-binding-destinationrule \
generate-runner-binding-podmonitor

generate-runner-binding-prometheusrule: download-crd-pr
  just generate-runner-binding tests/generated/monitoring.coreos.com_prometheusrules.yaml prometheusrules.monitoring.coreos.com prometheusrule.rs

generate-runner-binding-servicemonitor:
  just generate-runner-binding tests/servicemon-crd.yaml servicemonitors.monitoring.coreos.com servicemonitor.rs -d

generate-runner-binding-multiversion:
  just generate-runner-binding tests/mv-crd.yaml multiversions.clux.dev multiversion.rs -A

generate-runner-binding-agent:
  just generate-runner-binding tests/agent-crd.yaml agents.agent-install.openshift.io agent.rs "-b -A"

generate-runner-binding-application: download-crd-argo
  just generate-runner-binding tests/generated/application-crd.yaml applications.argoproj.io application.rs

generate-runner-binding-certificate: download-crd-certmanager
  just generate-runner-binding tests/generated/cert-manager.crds.yaml certificates.cert-manager.io certificate.rs -d

generate-runner-binding-cluster:
  just generate-runner-binding tests/cluster-crd.yaml clusters.cluster.x-k8s.io cluster.rs -d

generate-runner-binding-httproute:
  just generate-runner-binding tests/httproute-crd.yaml httproutes.gateway.networking.k8s.io httproute.rs

generate-runner-binding-serverauthorization:
  just generate-runner-binding tests/serverauth-crd.yaml serverauthorizations.policy.linkerd.io serverauthorization.rs -d

generate-runner-binding-destinationrule:
  just generate-runner-binding tests/destinationrule-crd.yaml destinationrules.networking.istio.io destinationrule.rs

generate-runner-binding-podmonitor:
  just generate-runner-binding tests/podmon-crd.yaml podmonitors.monitoring.coreos.com podmonitor.rs

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
