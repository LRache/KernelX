mod virtio;
mod device;
mod matcher;
mod manager;

pub mod block;

pub use manager::DEVICE_MANAGER as MANAGER;
use device::{Device, DeviceType};

use alloc::boxed::Box;

pub trait DriverOps {
    fn name(&self) -> &str;
    fn device_type(&self) -> DeviceType;

    fn as_block_driver(self: Box<Self>) -> Box<dyn block::BlockDriver> {
        unreachable!()
    }
}

pub trait DriverMatcher {
    fn try_match(&self, device: &Device) -> Option<Box<dyn DriverOps>>;
}

pub fn init() {
    // MANAGER.register_block_device("virtio_block0", Box::new(VirtIOBlockDevice::new(0x10001000)));
    
}
