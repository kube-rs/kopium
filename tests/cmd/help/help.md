```
$ kopium --help
Kubernetes OPenapI UnMangler

Usage: kopium [OPTIONS] [CRD]

Arguments:
  [CRD]
          Give the name of the input CRD to use (e.g., `prometheusrules.monitoring.coreos.com`)

Options:
  -f, --filename <FILE>
          Point to the location of a CRD to use on disk

  -A, --auto
          Enable all automation features
          
          This is a recommended, but early set of features that generates the most rust native code.
          
          It contains an unstable set of features and may get expanded in the future.
          
          Setting --auto enables: --schema=derived --derive=JsonSchema --docs

      --api-version <API_VERSION>
          Use this CRD version if multiple versions are present

      --hide-prelude
          Do not emit prelude(s)

      --hide-kube
          Do not derive CustomResource nor set kube-derive attributes
          
          If this is set, it makes any kube-derive specific options such as `--schema` unnecessary

  -d, --docs
          Emit doc comments from CRD field descriptions

      --builders
          Emit builder derives via the `typed-builder` crate

      --schema <SCHEMA_MODE>
          Schema mode to use for kube-derive
          
          The default is `disabled` and will compile without a schema, though the resulting CRD cannot be applied directly to a cluster.
          
          --schema=manual requires the user to `impl JsonSchema for <generated type>` elsewhere for the code to compile. Once this is done, the crd via `kube::CustomResourceExt::crd()` can be applied to a cluster directly.
          
          --schema=derived implies `--derive JsonSchema`. The resulting schema will compile without external user action. The crd via `CustomResourceExt::crd()` can be applied into Kubernetes directly.
          
          See: https://docs.rs/kube/latest/kube/derive.CustomResource.html#kubeschema--mode and https://docs.rs/kube/latest/kube/trait.CustomResourceExt.html#tymethod.crd
          
          [default: disabled]
          [possible values: manual, derived, disabled]

  -D, --derive <TRAIT>
          Derive these additional traits on generated objects
          
          There are three different ways of specifying traits to derive:
          
          1. A plain trait name will implement the trait for *all* objects generated from the custom resource definition: `--derive PartialEq`
          
          2. Constraining the derivation to a singular struct or enum: `--derive IssuerAcmeSolversDns01CnameStrategy=PartialEq`
          
          3. Constraining the derivation to only structs (@struct), enums (@enum) or *unit-only* enums (@enum:simple), meaning enums where no variants are tuple or structs: `--derive @struct=PartialEq`, `--derive @enum=PartialEq`, `--derive @enum:simple=PartialEq`
          
          See also: https://doc.rust-lang.org/reference/items/enumerations.html

  -e, --elide <ELIDE>
          Elide the following containers from the output
          
          This allows manual customization of structs from the output without having to remove it from the output first. Takes precise generated struct names.

      --relaxed
          Relaxed interpretation
          
          This allows certain invalid openapi specs to be interpreted as arbitrary objects as used by argo workflows, for example.

      --no-condition
          Disable standardized Condition API
          
          By default, kopium detects Condition objects and uses a standard Condition API from k8s_openapi instead of generating a custom definition.

      --no-object-reference
          Disable standardised ObjectReference API
          
          By default, kopium detects ObjectReference objects and uses a standard ObjectReference from k8s_openapi instead of generating a custom definition.

      --map-type <MAP_TYPE>
          Type used to represent maps via `additionalProperties`
          
          [default: BTreeMap]
          [possible values: BTreeMap, HashMap]

      --smart-derive-elision
          Automatically removes `#[derive(Default)]` from structs that contain fields for which a default cannot be automatically derived.
          
          This option only has an effect if `--derive Default` is set.

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

```