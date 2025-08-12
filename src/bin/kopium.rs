use std::{path::PathBuf, str::FromStr};

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kopium::{Derive, KopiumTypeGenerator, MapType};
use kube::{api, Api, Client, ResourceExt};

#[derive(Default, Parser)]
#[command(
    version = clap::crate_version!(),
    author = "clux <sszynrae@gmail.com>",
    about = "Kubernetes OPenapI UnMangler",
)]
struct Kopium {
    /// Give the name of the input CRD to use e.g. prometheusrules.monitoring.coreos.com
    #[arg(conflicts_with("file"))]
    crd: Option<String>,

    /// Point to the location of a CRD to use on disk
    #[arg(long = "filename", short, conflicts_with("crd"))]
    file: Option<PathBuf>,

    /// Use this CRD version if multiple versions are present
    #[arg(long)]
    api_version: Option<String>,

    /// Do not emit prelude(s)
    #[arg(long)]
    hide_prelude: bool,

    /// Do not derive CustomResource nor set kube-derive attributes
    ///
    /// If this is set, it makes any kube-derive specific options such as `--schema` unnecessary
    #[arg(long)]
    hide_kube: bool,

    /// Emit doc comments from descriptions
    #[arg(long, short)]
    docs: bool,

    /// Emit builder derives via the typed_builder crate
    #[arg(long, short)]
    builders: bool,

    /// Schema mode to use for kube-derive
    ///
    /// The default is --schema=disabled and will compile without a schema,
    /// but the resulting crd cannot be applied into a cluster.
    ///
    /// --schema=manual requires the user to `impl JsonSchema for MyCrdSpec` elsewhere for the code to compile.
    /// Once this is done, the crd via `CustomResourceExt::crd()` can be applied into Kubernetes directly.
    ///
    /// --schema=derived implies `--derive JsonSchema`. The resulting schema will compile without external user action.
    /// The crd via `CustomResourceExt::crd()` can be applied into Kubernetes directly.
    #[arg(
        long,
        default_value = "disabled",
        value_parser = ["disabled", "manual", "derived"],
    )]
    schema: String,

    /// Derive these additional traits on generated objects
    ///
    /// There are three different ways of specifying traits to derive:
    ///
    /// 1. A plain trait name will implement the trait for *all* objects generated from
    ///    the custom resource definition: `--derive PartialEq`
    ///
    /// 2. Constraining the derivation to a singular struct or enum:
    ///    `--derive IssuerAcmeSolversDns01CnameStrategy=PartialEq`
    ///
    /// 3. Constraining the derivation to only structs (@struct), enums (@enum) or *unit-only* enums (@enum:simple),
    ///    meaning enums where no variants are tuple or structs:
    ///    `--derive @struct=PartialEq`, `--derive @enum=PartialEq`, `--derive @enum:simple=PartialEq`
    ///
    /// See also: https://doc.rust-lang.org/reference/items/enumerations.html
    #[arg(long,
        short = 'D',
        value_parser = Derive::from_str,
    )]
    derive: Vec<Derive>,

    #[command(subcommand)]
    command: Option<Command>,

    /// Enable all automation features
    ///
    /// This is a recommended, but early set of features that generates the most rust native code.
    ///
    /// It contains an unstable set of features and may get expanded in the future.
    ///
    /// Setting --auto enables: --schema=derived --derive=JsonSchema --docs
    #[arg(long, short = 'A')]
    auto: bool,

    /// Elide the following containers from the output
    ///
    /// This allows manual customization of structs from the output without having to remove it from
    /// the output first. Takes precise generated struct names.
    #[arg(long, short = 'e')]
    elide: Vec<String>,

    /// Relaxed interpretation
    ///
    /// This allows certain invalid openapi specs to be interpreted as arbitrary objects as used by
    /// argo workflows, for example.
    #[arg(long)]
    relaxed: bool,

    /// Disable standardized Condition API
    ///
    /// By default, kopium detects Condition objects and uses a standard
    /// Condition API from k8s_openapi instead of generating a custom definition.
    #[arg(long)]
    no_condition: bool,

    /// Disable standardised ObjectReference API
    ///
    /// By default, kopium detects ObjectReference objects and uses a standard
    /// ObjectReference from k8s_openapi instead of generating a custom definition.
    #[arg(long)]
    no_object_reference: bool,

    /// Type used to represent maps via additionalProperties
    #[arg(long, value_enum, default_value_t)]
    map_type: MapType,

    /// Automatically removes `#[derive(Default)]` from structs that contain fields for
    /// which a default cannot be automatically derived.
    ///
    /// This option only has an effect if `--derive Default` is set.
    #[arg(long)]
    smart_derive_elision: bool,
}

#[derive(Clone, Copy, Debug, Subcommand)]
#[command(args_conflicts_with_subcommands = true)]
enum Command {
    #[command(about = "List available CRDs", hide = true)]
    ListCrds,
    #[command(about = "Generate completions", hide = true)]
    Completions {
        #[arg(help = "The shell to generate completions for")]
        shell: clap_complete::Shell,
    },
}

impl Kopium {
    async fn dispatch(self) -> anyhow::Result<()> {
        if let Some(name) = self.crd.as_deref() {
            let api = Client::try_default()
                .await
                .map(Api::<CustomResourceDefinition>::all)?;

            let crd = api.get(name).await?;
            let args = std::env::args().skip(1).collect::<Vec<_>>().join(" ");

            let generated = KopiumTypeGenerator::from(self)
                .generate_rust_types_for(&crd, Some(args))
                .await?;

            println!("{}", generated);

            Ok(())
        } else if let Some(file) = self.file.as_deref() {
            // no cluster access needed in this case
            let data = if file.to_string_lossy() == "-" {
                get_stdin_data().with_context(|| "Failed to read from stdin".to_string())?
            } else {
                std::fs::read_to_string(file).with_context(|| format!("Failed to read {}", file.display()))?
            };

            let crd: CustomResourceDefinition = serde_yaml::from_str(&data)?;
            let args = std::env::args().skip(1).collect::<Vec<_>>().join(" ");

            let generated = KopiumTypeGenerator::from(self)
                .generate_rust_types_for(&crd, Some(args))
                .await?;

            println!("{}", generated);

            Ok(())
        } else if let Some(command) = self.command {
            match command {
                Command::ListCrds => {
                    let api = Client::try_default()
                        .await
                        .map(Api::<CustomResourceDefinition>::all)?;

                    self.list_crds(api).await
                }
                Command::Completions { shell } => self.completions(shell),
            }
        } else {
            self.help()
        }
    }

    async fn list_crds(&self, api: Api<CustomResourceDefinition>) -> anyhow::Result<()> {
        let params = api::ListParams::default();

        api.list(&params).await?.items.iter().for_each(|crd| {
            println!("{}", crd.name_any());
        });

        Ok(())
    }

    fn completions(&self, shell: clap_complete::Shell) -> anyhow::Result<()> {
        let mut command = Self::command();

        clap_complete::generate(shell, &mut command, "kopium", &mut std::io::stdout());

        Ok(())
    }

    fn help(&self) -> anyhow::Result<()> {
        Self::command().print_help().map_err(Into::into)
    }
}

impl From<Kopium> for KopiumTypeGenerator {
    fn from(value: Kopium) -> Self {
        KopiumTypeGenerator::builder()
            .docs(value.docs)
            .elide(value.elide)
            .schema(value.schema)
            .relaxed(value.relaxed)
            .builders(value.builders)
            .map_type(value.map_type)
            .derive_all(value.derive)
            .hide_kube(value.hide_kube)
            .api_version(value.api_version)
            .hide_prelude(value.hide_prelude)
            .no_condition(value.no_condition)
            .no_object_reference(value.no_object_reference)
            .smart_derive_elision(value.smart_derive_elision)
            .build()
    }
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Ignore SIGPIPE errors to avoid having to use let _ = write! everywhere
    // See https://github.com/rust-lang/rust/issues/46016
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let mut args = <Kopium as clap::Parser>::parse();

    if args.auto {
        args.docs = true;
        args.schema = "derived".into();
    }

    args.dispatch().await
}

fn get_stdin_data() -> anyhow::Result<String> {
    use std::io::{stdin, Read};

    let mut buf = Vec::new();

    stdin().read_to_end(&mut buf)?;

    let input = String::from_utf8(buf)?;

    Ok(input)
}
