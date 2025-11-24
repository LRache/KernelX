use alloc::sync::Arc;

use super::manager;
use super::{Device, DriverOps};

use super::block::starfive_sdio;
use super::virtio::VirtIODriverMatcher;

pub trait DriverMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>>;
}

static VIRTIO: VirtIODriverMatcher = VirtIODriverMatcher::new();
static VF_SDIO: starfive_sdio::Matcher = starfive_sdio::Matcher::new();

pub fn register_matchers() {
    manager::register_matcher(&VIRTIO);
    manager::register_matcher(&VF_SDIO);
}
