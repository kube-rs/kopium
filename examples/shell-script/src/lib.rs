//! This library is exporting generated types from the `kopium` build script.
//!
//! For more information, see the [kopium examples](https://github.com/kube-rs/kopium/blob/main/examples/)

#[allow(unused_imports)]
mod crds;

pub use crds::*;

// Note: anything added to or changed in the crds/ folder
// will be overwritten by the generation step in the just file
// so any extension methods/impls for the generated types should be added outside that folder

impl servicemonitor::ServiceMonitorSpec {
    /// A custom method for one of the generated types that won't be overwritten by the build script
    pub fn some_custom_method(&self) -> bool {
        todo!()
    }
}
