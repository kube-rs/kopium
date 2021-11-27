use anyhow::Result;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition
};
use kube::{Api, Client, Resource, ResourceExt};

include!("./gen.rs"); // import generated test structs in scope

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::try_default().await?;

    let api: Api<CustomResourceDefinition> = Api::all(client.clone());
    let cr: Api<CR> = Api::default_namespaced(client);

    println!("crd gvk {}-{}-{}", CR::group(&()), CR::version(&()), CR::kind(&()));
    let canonical = api.get(&format!("{}.{}", CR::plural(&()), CR::group(&()))).await?;
    assert_eq!(canonical.spec.names.kind, CR::kind(&()).to_string());
    assert_eq!(canonical.spec.names.plural, CR::plural(&()).to_string());
    assert_eq!(canonical.spec.group, CR::group(&()).to_string());

    // assumes a resource of type CR has been applied with name 'gen' in the namespace
    let crd = cr.get("gen").await?;
    assert_eq!(crd.name(), "gen");

    Ok(())
}
