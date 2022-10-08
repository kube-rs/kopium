#![allow(non_snake_case)]
#[cfg(test)]
mod tests {
    include!("./gen.rs"); // import generated test structs in scope

    use anyhow::Result;
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
    use kube::{
        api::{Api, Patch, PatchParams},
        Client, Resource, ResourceExt,
    };

    #[tokio::test]
    async fn verify_gen() -> Result<()> {
        let client = Client::try_default().await?;

        let api: Api<CustomResourceDefinition> = Api::all(client.clone());
        let cr: Api<CR> = Api::default_namespaced(client);

        println!(
            "# crd gvk {}-{}-{}",
            CR::group(&()),
            CR::version(&()),
            CR::kind(&())
        );
        let canonical = api
            .get(&format!("{}.{}", CR::plural(&()), CR::group(&())))
            .await?;
        assert_eq!(canonical.spec.names.kind, CR::kind(&()).to_string());
        assert_eq!(canonical.spec.names.plural, CR::plural(&()).to_string());
        assert_eq!(canonical.spec.group, CR::group(&()).to_string());

        // assumes a resource of type CR has been applied with name 'gen' in the namespace
        println!(
            "# Api<{}.{}>.get(\"{}\")",
            canonical.spec.names.kind, canonical.spec.group, "gen"
        );
        let instance = cr.get("gen").await?;
        assert_eq!(instance.name_unchecked(), "gen");

        // extra verification for status types - replace_status manually
        let filename = format!("./tests/{}.yaml", CR::kind(&()).to_string().to_ascii_lowercase());
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
}
