//! This library is exporting generated types from the `kopium` build script.
//!
//! For more information, see the [kopium examples](https://github.com/kube-rs/kopium/blob/main/examples/)

mod crds;

#[allow(unused_imports)] pub use crds::*;

// Note: anything added to or changed in the `crds` modules (i.e. `crds.rs`, *or* `crds/*.rs`)
// will be overwritten by the build script, so any extension methods/impls for the generated types should be added outside the crds module.

impl servicemonitor::ServiceMonitor {
    /// A custom method for one of the generated types that won't be overwritten by the build script
    pub fn some_custom_method(&self) -> bool {
        todo!()
    }
}
