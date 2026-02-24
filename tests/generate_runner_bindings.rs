use anyhow::{anyhow, Context, Result};
use envtest::Environment;
use serde::Deserialize;
use std::{env, fs, path::Path, process::Command};

fn required_env(key: &str) -> Result<String> {
    env::var(key).with_context(|| format!("missing required env var {}", key))
}

fn read_crds_yaml(source: &str) -> Result<serde_yaml::Value> {
    let documents: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(source)
        .map(serde_yaml::Value::deserialize)
        .collect::<Result<_, _>>()?;
    Ok(serde_yaml::Value::Sequence(documents))
}

#[ignore]
#[test]
fn generate_runner_bindings() -> Result<()> {
    let crd_path = required_env("RUNNER_GEN_CRD_PATH")?;
    let resource = required_env("RUNNER_GEN_RESOURCE")?;
    let out_dir = required_env("RUNNER_GEN_OUT_DIR")?;
    let out_file = required_env("RUNNER_GEN_OUT_FILE")?;
    let extra_args = env::var("RUNNER_GEN_EXTRA_ARGS").unwrap_or_default();

    let source = fs::read_to_string(&crd_path).with_context(|| format!("reading CRD file {}", crd_path))?;
    let crds = read_crds_yaml(&source).with_context(|| format!("failed to read CRDs from {}", crd_path))?;
    let env = Environment::default().with_crds(crds)?;
    let server = env
        .create()
        .with_context(|| format!("starting envtest for {}", resource))?;

    let kubeconfig = server.kubeconfig()?;
    let kubeconfig_path = format!("target/tmp/runner-gen-{}.kubeconfig", std::process::id());
    if let Some(dir) = Path::new(&kubeconfig_path).parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&kubeconfig_path, serde_yaml::to_string(&kubeconfig)?)?;

    let mut cmd = Command::new("cargo");
    cmd.arg("run").arg("--quiet").arg("--bin").arg("kopium").arg("--");
    for arg in extra_args.split_whitespace() {
        cmd.arg(arg);
    }
    cmd.arg(&resource);
    cmd.env("KUBECONFIG", &kubeconfig_path);

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
