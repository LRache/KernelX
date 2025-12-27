use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::{fs::devfs, kinfo, kwarn};

use super::{DriverMatcher, Device, DriverOps, BlockDriverOps, CharDriverOps, RTCDriverOps};

static MATCHERS: RwLock<Vec<&'static dyn DriverMatcher>> = RwLock::new(Vec::new());
static INTERRUPT_MAP: RwLock<BTreeMap<u32, Arc<dyn DriverOps>>> = RwLock::new(BTreeMap::new());
static DRIVERS: RwLock<BTreeMap<String, Arc<dyn DriverOps>>> = RwLock::new(BTreeMap::new());

fn try_match(device: &Device) -> Option<Arc<dyn DriverOps>> {
    for matcher in MATCHERS.read().iter() {
        if let Some(driver) = matcher.try_match(device) {
            kinfo!("Matched driver: {} for device: {:02x?}", driver.name(), device);
            return Some(driver);
        }
    }
    kwarn!("No driver found for device: {:02x?}", device);
    None
}

pub fn register_matcher(matcher: &'static dyn DriverMatcher) {
    MATCHERS.write().push(matcher);
}

pub fn found_device(device: &Device) {
    if let Some(driver) = try_match(device) {
        let name = driver.device_name();
        if let Some(irq) = device.interrupt_number() {
            INTERRUPT_MAP.write().insert(irq, driver.clone());
        }

        DRIVERS.write().insert(name.clone(), driver.clone());
        
        devfs::add_device(name, driver);
    }
}

pub fn register_matched_driver(driver: Arc<dyn DriverOps>) {
    let name = driver.device_name();
    DRIVERS.write().insert(name, driver);
}

pub fn get_block_driver(name: &str) -> Option<Arc<dyn BlockDriverOps>> {
    DRIVERS
        .read()
        .get(name)
        .cloned()
        .and_then(|driver| driver.as_block_driver())
}

pub fn get_char_driver(name: &str) -> Option<Arc<dyn CharDriverOps>> {
    DRIVERS
        .read()
        .get(name)
        .cloned()
        .and_then(|driver| driver.as_char_driver())
}

pub fn get_rtc_driver(name: &str) -> Option<Arc<dyn RTCDriverOps>> {
    DRIVERS
        .read()
        .get(name)
        .cloned()
        .and_then(|driver| driver.as_rtc_driver())
}

pub fn handle_interrupt(irq: u32) {
    if let Some(driver) = INTERRUPT_MAP.read().get(&irq) {
        driver.handle_interrupt();
    } else {
        kwarn!("No driver registered for interrupt {}", irq);
    }
}
