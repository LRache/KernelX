use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::RwLock;

use crate::driver::block::BlockDriver;

use super::{DriverMatcher, Device, DriverOps, DeviceType};
use super::block::BlockDevice;

pub struct DriverManager {
    matchers: RwLock<Vec<Box<dyn DriverMatcher>>>,
    block: RwLock<BTreeMap<String, Box<dyn BlockDriver>>>,
}

impl DriverManager {
    pub const fn new() -> Self {
        DriverManager {
            matchers: RwLock::new(Vec::new()),
            block: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn register_matcher(&self, matcher: Box<dyn DriverMatcher>) {
        self.matchers.write().push(matcher);
    }

    pub fn try_match(&self, device: &Device) -> Option<Box<dyn DriverOps>> {
        for matcher in self.matchers.read().iter() {
            if let Some(driver) = matcher.try_match(device) {
                return Some(driver);
            }
        }
        None
    }

    pub fn register_device(&self, device: &Device) {
        if let Some(driver) = self.try_match(device) {
            match driver.device_type() {
                DeviceType::Block => {
                    self.register_block_device(driver.as_block_driver());
                }
                _ => {}
            }
        }
    }

    pub fn register_block_device(&self, device: Box<dyn BlockDriver>) {
        self.block.write().insert(device.name().into(), device);
    }

    pub fn get_block_driver(&self, name: &str) -> Option<Box<dyn BlockDriver>> {
        self.block
            .read()
            .get(name)
            .map(|driver| driver.clone_boxed())
    }
}

unsafe impl Sync for DriverManager {}
unsafe impl Send for DriverManager {}

pub static DEVICE_MANAGER: DriverManager = DriverManager::new();
