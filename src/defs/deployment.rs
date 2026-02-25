use rhai::{CustomType, TypeBuilder};

use super::Holder;

#[derive(Debug, Default, Clone)]
pub struct DeploymentDef {}

#[derive(Debug, Default, Clone)]
pub struct Deployment(Holder<DeploymentDef>);

impl CustomType for Deployment {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Deployment")
            // .with_fn("host", |this: &mut Self, host: &str| {
            //     this.0.lock().unwrap().host = Some(host.into());
            //     this.clone()
            // })
        ;
    }
}
