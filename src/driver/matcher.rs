use alloc::sync::Arc;

use super::manager;
use super::{Device, DriverOps};
use super::{char, block, virtio};

pub trait DriverMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>>;
}

pub fn register_matchers() {
    manager::register_matcher(&virtio::Matcher);
    manager::register_matcher(&char::uart16650::Matcher);
    manager::register_matcher(&block::starfive_sdio::Matcher);
}
