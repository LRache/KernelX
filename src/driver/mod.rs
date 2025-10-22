mod virtio;
mod device;
mod matcher;
mod manager;

pub mod block;

use device::{Device, DeviceType};
use matcher::DriverMatcher;

pub use manager::{get_block_driver, load_device_tree};

use alloc::sync::Arc;
use alloc::string::String;

pub trait DriverOps {
    fn name(&self) -> &str;

    fn device_name(&self) -> String;
    fn device_type(&self) -> DeviceType;

    fn as_block_driver(self: Arc<Self>) -> Arc<dyn block::BlockDriver> {
        unreachable!()
    }
}

pub fn init() {    
    matcher::register_matchers();
}
