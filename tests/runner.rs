#![allow(non_snake_case)]
#[cfg(test)]
#[allow(warnings)]
mod tests {
    // promstack
    mod prometheusrule {
        include!("./generated/prometheusrule.rs");
    }
    mod podmonitor {
        include!("./generated/podmonitor.rs");
    }
    mod servicemonitor {
        include!("./generated/servicemonitor.rs");
    }

    // ad-hoc versioning
    mod multiversion {
        include!("./generated/multiversion.rs");
    }

    // openshift
    mod agent {
        include!("./generated/agent.rs");
    }

    // argo
    mod application {
        include!("./generated/application.rs");
    }

    // certmanager
    mod certificate {
        include!("./generated/certificate.rs");
    }

    // istio
    mod destinationrule {
        include!("./generated/destinationrule.rs");
    }

    // gatewayapi
    mod httproute {
        include!("./generated/httproute.rs");
    }

    // linkerd
    mod serverauthorization {
        include!("./generated/serverauthorization.rs");
    }

    use agent::*;
    use application::*;
    use certificate::*;
    use destinationrule::*;
    use httproute::*;
    use multiversion::*;
    use podmonitor::*;
    use prometheusrule::*;
    use serverauthorization::*;
    use servicemonitor::*;

    use anyhow::Result;
    use envtest::Environment;
    use k8s_openapi::{
        apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
        NamespaceResourceScope,
    };
    use kube::{
        api::{Api, Patch, PatchParams, PostParams},
        ResourceExt,
    };
    use serde::{de::DeserializeOwned, Deserialize, Serialize};
    use std::fmt::Debug;

    fn load_crd_from_env(path: String) -> Result<serde_yaml::Value> {
        let contents = std::fs::read_to_string(path)?;
        let documents: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(&contents)
            .map(serde_yaml::Value::deserialize)
            .collect::<Result<_, _>>()?;
        Ok(serde_yaml::Value::Sequence(documents))
    }

    fn load_resource_from_env<CR>(path: String) -> Result<CR>
    where
        CR: DeserializeOwned,
    {
        let contents = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&contents)?)
    }

    async fn verify_gen<CR>(crds: serde_yaml::Value, resource: CR) -> Result<()>
    where
        CR::DynamicType: Default,
        CR: ResourceExt<Scope = NamespaceResourceScope>,
        CR: Clone + Debug + Serialize + DeserializeOwned,
    {
        let env = Environment::default().with_crds(crds)?.create()?;
        let client = env.client()?;

        let api: Api<CustomResourceDefinition> = Api::all(client.clone());
        let cr: Api<CR> = Api::default_namespaced(client);

        println!(
            "# crd gvk {}-{}-{}",
            CR::group(&Default::default()),
            CR::version(&Default::default()),
            CR::kind(&Default::default()),
        );
        let canonical = api
            .get(&format!(
                "{}.{}",
                CR::plural(&Default::default()),
                CR::group(&Default::default())
            ))
            .await?;
        assert_eq!(
            canonical.spec.names.kind,
            CR::kind(&Default::default()).to_string()
        );
        assert_eq!(
            canonical.spec.names.plural,
            CR::plural(&Default::default()).to_string()
        );
        assert_eq!(canonical.spec.group, CR::group(&Default::default()).to_string());

        cr.create(&PostParams::default(), &resource).await?;

        // assumes a resource of type CR has been applied with name 'gen' in the namespace
        println!(
            "# Api<{}.{}>.get(\"gen\")",
            canonical.spec.names.kind, canonical.spec.group
        );
        let instance = cr.get("gen").await?;
        assert_eq!(instance.name_unchecked(), "gen");

        // extra verification for status types - replace_status manually
        let filename = format!(
            "./tests/{}.yaml",
            CR::kind(&Default::default()).to_string().to_ascii_lowercase()
        );
        // NB: this relies on filenames following a format, and having a status object
        println!("# speculatively opening '{}' for replacing", filename);
        if let Ok(contents) = std::fs::read_to_string(&filename) {
            let file_data: serde_yaml::Value = serde_yaml::from_str(&contents).expect("read yaml");
            let data: serde_json::Value = serde_json::to_value(&file_data).expect("to json");
            if let Some(root) = data.as_object() {
                if root.contains_key("status") {
                    println!("# patching status");
                    let patch = Patch::Merge(data);

                    let pp = PatchParams::default();
                    let _obj = cr.patch_status("gen", &pp, &patch).await?;
                    // TODO: need some generic way to detect if we can use status..
                    //assert_eq!(obj.status.is_some());
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn verify_openshift_agent() -> Result<()> {
        verify_gen::<Agent>(
            load_crd_from_env("tests/agent-crd.yaml".into())?,
            load_resource_from_env("tests/agent.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_argo_application() -> Result<()> {
        verify_gen::<Application>(
            load_crd_from_env("tests/generated/application-crd.yaml".into())?,
            load_resource_from_env("tests/app.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_certman_certificate() -> Result<()> {
        verify_gen::<Certificate>(
            load_crd_from_env("tests/certificates.cert-manager.io.yaml".into())?,
            load_resource_from_env("tests/cert.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_istio_destinationrule() -> Result<()> {
        verify_gen::<DestinationRule>(
            load_crd_from_env("tests/destinationrule-crd.yaml".into())?,
            load_resource_from_env("tests/destinationrule.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_gateway_route() -> Result<()> {
        verify_gen::<HTTPRoute>(
            load_crd_from_env("tests/httproute-crd.yaml".into())?,
            load_resource_from_env("tests/httproute.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_multiversion_crd() -> Result<()> {
        verify_gen::<MultiVersion>(
            load_crd_from_env("tests/mv-crd.yaml".into())?,
            load_resource_from_env("tests/mv.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_promstack_podmonitor() -> Result<()> {
        verify_gen::<PodMonitor>(
            load_crd_from_env("tests/podmonitors.monitoring.coreos.com.yaml".into())?,
            load_resource_from_env("tests/podmon.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_promstack_promrule() -> Result<()> {
        verify_gen::<PrometheusRule>(
            load_crd_from_env("tests/prometheusrules.monitoring.coreos.com.yaml".into())?,
            load_resource_from_env("tests/pr.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_promstack_servicemon() -> Result<()> {
        verify_gen::<ServiceMonitor>(
            load_crd_from_env("tests/servicemonitors.monitoring.coreos.com.yaml".into())?,
            load_resource_from_env("tests/servicemon.yaml".into())?,
        )
        .await
    }

    #[tokio::test]
    async fn verify_linkerd_serverauth() -> Result<()> {
        verify_gen::<ServerAuthorization>(
            load_crd_from_env("tests/serverauth-crd.yaml".into())?,
            load_resource_from_env("tests/serverauth.yaml".into())?,
        )
        .await
    }
}
