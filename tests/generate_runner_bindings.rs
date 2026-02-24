use anyhow::{anyhow, Context, Result};
use std::{env, fs, path::Path, process::Command};

fn required_env(key: &str) -> Result<String> {
    env::var(key).with_context(|| format!("missing required env var {}", key))
}

#[ignore]
#[test]
fn generate_runner_bindings() -> Result<()> {
    let resource = required_env("RUNNER_GEN_RESOURCE")?;
    let out_dir = required_env("RUNNER_GEN_OUT_DIR")?;
    let out_file = required_env("RUNNER_GEN_OUT_FILE")?;
    let extra_args = env::var("RUNNER_GEN_EXTRA_ARGS").unwrap_or_default();

    let mut cmd = Command::new("cargo");
    cmd.arg("run").arg("--quiet").arg("--bin").arg("kopium").arg("--");
    for arg in extra_args.split_whitespace() {
        cmd.arg(arg);
    }
    cmd.arg(&resource);

    let output = cmd
        .output()
        .with_context(|| format!("running kopium for {}", resource))?;
    if !output.status.success() {
        return Err(anyhow!(
            "kopium failed for {}: {}",
            resource,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    fs::create_dir_all(&out_dir)?;
    let out_path = Path::new(&out_dir).join(&out_file);
    fs::write(out_path, output.stdout)?;

    Ok(())
}
