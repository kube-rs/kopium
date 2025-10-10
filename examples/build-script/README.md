# Build Script Generated Library

Uses the prometheus operator crds (the single bundle yaml in [a release](https://github.com/prometheus-operator/prometheus-operator/releases/tag/v0.86.0)).

The build script will re-generate these types when the yaml file changes.

## Maximal
This is a more complicated, but more flexible setup that allows injecting things with rust source code (in the `build.rs`).

It also generates with everything on (docs / builders / kube compat).
