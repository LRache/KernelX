use alloc::sync::Arc;

use super::manager;
use super::{Device, DriverOps};

use super::virtio::VirtIODriverMatcher;

pub trait DriverMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>>;
}

static VIRTIO: VirtIODriverMatcher = VirtIODriverMatcher::new();

pub fn register_matchers() {
    manager::register_matcher(&VIRTIO);
}
