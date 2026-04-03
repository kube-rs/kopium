#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(not(feature = "cli"))]
    anyhow::bail!("please build `kopium` with the 'cli' feature enabled");

    #[cfg(feature = "cli")]
    cli::kopium_cli().await
}

#[cfg(feature = "cli")]
mod cli {
    use std::path::PathBuf;

    use anyhow::Context;
    use clap::CommandFactory;
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;

    #[derive(clap::Parser)]
    #[command(
        version = clap::crate_version!(),
        author = "clux <sszynrae@gmail.com>",
        about = "Kubernetes OPenapI UnMangler",
    )]
    struct Kopium {
        /// Give the name of the input CRD to use (e.g., `prometheusrules.monitoring.coreos.com`)
        #[arg(conflicts_with("file"))]
        crd: Option<String>,

        /// Point to the location of a CRD to use on disk
        #[arg(long = "filename", short, conflicts_with("crd"))]
        file: Option<PathBuf>,

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

        #[command(flatten)]
        generator: kopium::TypeGenerator,
    }

    #[derive(Clone, Copy, Debug, clap::Subcommand)]
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

    pub async fn kopium_cli() -> anyhow::Result<()> {
        env_logger::init();
        // Ignore SIGPIPE errors to avoid having to use let _ = write! everywhere
        // See https://github.com/rust-lang/rust/issues/46016
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }

        let mut args: Kopium = clap::Parser::parse();

        if args.auto {
            args.generator.emit_docs = true;
            args.generator.schema_mode = kopium::SchemaMode::Derived;
        }

        if args.generator.schema_mode == kopium::SchemaMode::Derived {
            let json_schema = kopium::Derive::all("JsonSchema");

            if !args.generator.derive_traits.contains(&json_schema) {
                args.generator.derive_traits.push(json_schema)
            }
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

    impl Kopium {
        async fn dispatch(&self) -> anyhow::Result<()> {
            if let Some(name) = self.crd.as_deref() {
                return self.generate_types_for_fetched_crd(name).await;
            }

            if let Some(file) = self.file.as_deref() {
                return self.generate_types_for_file(file).await;
            }

            match self.command {
                None => self.help(),
                Some(Command::ListCrds) => self.list_crds().await,
                Some(Command::Completions { shell }) => self.completions(shell),
            }
        }

        fn help(&self) -> anyhow::Result<()> {
            Self::command().print_help()?;
            Ok(())
        }

        fn completions(&self, shell: clap_complete::Shell) -> anyhow::Result<()> {
            let mut command = Self::command();

            clap_complete::generate(shell, &mut command, "kopium", &mut std::io::stdout());

            Ok(())
        }

        async fn list_crds(&self) -> anyhow::Result<()> {
            let api = kube::Client::try_default()
                .await
                .map(kube::Api::<CustomResourceDefinition>::all)?;

            for crd_name in api
                .list(&Default::default())
                .await?
                .items
                .iter()
                .map(kube::ResourceExt::name_any)
            {
                println!("{crd_name}");
            }

            Ok(())
        }

        async fn generate_types_for(&self, crd: &CustomResourceDefinition) -> anyhow::Result<()> {
            let args = std::env::args().skip(1).collect::<Vec<_>>().join(" ");

            let generated = self.generator.generate_rust_types_for(crd, Some(args))?;

            println!("{generated}");

            Ok(())
        }

        async fn generate_types_for_file(&self, target: impl AsRef<std::path::Path>) -> anyhow::Result<()> {
            let target = target.as_ref();

            // no cluster access needed in this case
            let data = if target == <str as AsRef<std::path::Path>>::as_ref("-") {
                get_stdin_data().with_context(|| "Failed to read from stdin".to_string())?
            } else {
                std::fs::read_to_string(target)
                    .with_context(|| format!("Failed to read {}", target.display()))?
            };

            let crd = serde_yaml::from_str::<CustomResourceDefinition>(&data)?;

            self.generate_types_for(&crd).await
        }

        async fn generate_types_for_fetched_crd(&self, target: &str) -> anyhow::Result<()> {
            let api = kube::Client::try_default()
                .await
                .map(kube::Api::<CustomResourceDefinition>::all)?;

            let crd = api.get(target).await?;

            self.generate_types_for(&crd).await
        }
    }
}
