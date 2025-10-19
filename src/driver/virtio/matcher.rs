use alloc::boxed::Box;

use crate::driver::{Device, DriverOps, DriverMatcher};

pub struct VirtIODriverMatcher;

impl DriverMatcher for VirtIODriverMatcher {
    fn try_match(&self, device: &Device) -> Option<Box<dyn DriverOps>> {
        if device.typename == "virtio_mmio" && device.compatible == "virtio,mmio" {
            let header = 
        } else {
            None
        }
    }
}
