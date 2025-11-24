use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::{kinfo, kwarn};

use super::{BlockDriverOps, CharDriverOps, Device, DeviceType, DriverMatcher, DriverOps};

pub struct DriverManager {
    matchers: RwLock<Vec<&'static dyn DriverMatcher>>,
    block: RwLock<BTreeMap<String, Arc<dyn BlockDriverOps>>>,
    char: RwLock<BTreeMap<String, Arc<dyn CharDriverOps>>>,
}

impl DriverManager {
    const fn new() -> Self {
        DriverManager {
            matchers: RwLock::new(Vec::new()),
            block: RwLock::new(BTreeMap::new()),
            char: RwLock::new(BTreeMap::new()),
        }
    }

    fn register_matcher(&self, matcher: &'static dyn DriverMatcher) {
        self.matchers.write().push(matcher);
    }

    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        for matcher in self.matchers.read().iter() {
            if let Some(driver) = matcher.try_match(device) {
                return Some(driver);
            }
        }
        kwarn!("No driver found for device: {:?}", device);
        None
    }

    fn found_device(&self, device: &Device) {
        if let Some(driver) = self.try_match(device) {
            kinfo!(
                "Registering driver: {} for device {}: {:?}",
                driver.name(),
                driver.device_name(),
                device
            );
            match driver.device_type() {
                DeviceType::Block => {
                    self.register_block_device(driver.as_block_driver());
                }
                _ => {}
            }
        }
    }

    fn register_matched_driver(&self, driver: Arc<dyn DriverOps>) {
        match driver.device_type() {
            DeviceType::Block => {
                self.register_block_device(driver.as_block_driver());
            }
            DeviceType::Char => {
                self.register_char_device(driver.as_char_driver());
            }
            _ => unimplemented!(),
        }
    }

    fn register_block_device(&self, driver: Arc<dyn BlockDriverOps>) {
        self.block.write().insert(driver.device_name(), driver);
    }

    fn register_char_device(&self, driver: Arc<dyn CharDriverOps>) {
        self.char.write().insert(driver.device_name(), driver);
    }

    fn get_block_driver(&self, name: &str) -> Option<Arc<dyn BlockDriverOps>> {
        self.block.read().get(name).map(|driver| driver.clone())
    }

    fn get_char_driver(&self, name: &str) -> Option<Arc<dyn CharDriverOps>> {
        self.char.read().get(name).map(|driver| driver.clone())
    }
}

unsafe impl Sync for DriverManager {}

static DRIVER_MANAGER: DriverManager = DriverManager::new();

pub fn found_device(device: &Device) {
    DRIVER_MANAGER.found_device(device);
}

pub fn register_matcher(matcher: &'static dyn DriverMatcher) {
    DRIVER_MANAGER.register_matcher(matcher);
}

pub fn register_matched_driver(driver: Arc<dyn DriverOps>) {
    DRIVER_MANAGER.register_matched_driver(driver);
}

pub fn get_block_driver(name: &str) -> Option<Arc<dyn BlockDriverOps>> {
    DRIVER_MANAGER.get_block_driver(name)
}

pub fn get_char_driver(name: &str) -> Option<Arc<dyn CharDriverOps>> {
    DRIVER_MANAGER.get_char_driver(name)
}
