#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Ignore SIGPIPE errors to avoid having to use let _ = write! everywhere
    // See https://github.com/rust-lang/rust/issues/46016
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let mut args = <kopium::Kopium as clap::Parser>::parse();

    if args.auto {
        args.docs = true;
        args.schema = "derived".into();
    }
    if args.schema == "derived" {
        let json_schema = kopium::Derive::all("JsonSchema");

        if !args.derive.contains(&json_schema) {
            args.derive.push(json_schema)
        }
    }

    args.dispatch().await
}
