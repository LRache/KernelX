use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::boxed::Box;
use spin::RwLock;

use crate::driver::block::{BlockDriver, VirtIOBlockDevice};

use super::block::BlockDevice;

pub struct DriverManager {
    block: RwLock<BTreeMap<String, Box<dyn BlockDevice>>>,
}

impl DriverManager {
    pub const fn new() -> Self {
        DriverManager {
            block: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn register_block_device(&self, name: &str, device: Box<dyn BlockDevice>) {
        self.block.write().insert(name.into(), device);
    }

    pub fn get_block_driver(&self, name: &str) -> Option<Box<dyn BlockDriver>> {
        self.block.read().get(name).map(|driver| driver.driver())
    }
}

unsafe impl Sync for DriverManager {}
unsafe impl Send for DriverManager {}

pub static DEVICE_MANAGER: DriverManager = DriverManager::new();

pub fn init() {
    DEVICE_MANAGER.register_block_device("virtio_block0", Box::new(VirtIOBlockDevice::new(0x10001000)));
}
