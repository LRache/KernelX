use alloc::sync::Arc;

use super::manager;
use super::{Device, DriverOps};
use super::{char, block, virtio, rtc};

pub trait DriverMatcher: Send + Sync {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>>;
}
  
pub fn register_matchers() {
    manager::register_matcher(&virtio::Matcher);
    manager::register_matcher(&char::serial::ns16550a::Matcher);
    manager::register_matcher(&block::starfive_sdio::Matcher);
    manager::register_matcher(&rtc::goldfish::Matcher);
}
