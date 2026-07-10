use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::fs::devfs;
use crate::{arch, kinfo, kwarn};

use super::{BlockDriverOps, Device, DriverOps, MMIOMatcher, PCIDevice, PCIMatcher, RTCDriverOps};

static MMIO_MATCHERS: RwLock<Vec<&'static dyn MMIOMatcher>> = RwLock::new(Vec::new());
static PCI_MATCHERS: RwLock<Vec<&'static dyn PCIMatcher>> = RwLock::new(Vec::new());
static INTERRUPT_MAP: RwLock<BTreeMap<u32, Arc<dyn DriverOps>>> = RwLock::new(BTreeMap::new());
static DRIVERS: RwLock<BTreeMap<String, Arc<dyn DriverOps>>> = RwLock::new(BTreeMap::new());

fn try_match_mmio_device(device: &Device) -> Option<Arc<dyn DriverOps>> {
    for matcher in MMIO_MATCHERS.read().iter() {
        if let Some(driver) = matcher.try_match(device) {
            kinfo!("Matched driver: {} for device: {:02x?}", driver.name(), device);
            return Some(driver);
        }
    }
    kwarn!("No driver found for device: {:02x?}", device);
    None
}

fn try_match_pci_device(device: &mut PCIDevice) -> Option<Arc<dyn DriverOps>> {
    for matcher in PCI_MATCHERS.read().iter() {
        if let Some(driver) = matcher.try_match(device) {
            kinfo!(
                "Matched driver: {} for PCI device: {} {}",
                driver.name(),
                device.device_function(),
                device.info(),
            );
            return Some(driver);
        }
    }
    kwarn!(
        "No driver found for PCI device: {} {}",
        device.device_function(),
        device.info()
    );
    None
}

fn register_found_driver(driver: Arc<dyn DriverOps>, irq: Option<u32>) {
    let name = driver.device_name();
    if let Some(irq) = irq {
        INTERRUPT_MAP.write().insert(irq, driver.clone());
    }

    DRIVERS.write().insert(name.clone(), driver.clone());

    devfs::add_device(name, driver);
}

pub fn register_mmio_matcher(matcher: &'static dyn MMIOMatcher) {
    MMIO_MATCHERS.write().push(matcher);
}

pub fn register_pci_matcher(matcher: &'static dyn PCIMatcher) {
    PCI_MATCHERS.write().push(matcher);
}

pub fn found_device(device: &mut Device) {
    let is_pci = device.is_pci();
    let interrupt_number = device.interrupt_number();
    let driver = if let Some(pci_device) = device.pci_mut() {
        try_match_pci_device(pci_device)
    } else {
        try_match_mmio_device(device)
    };

    if let Some(driver) = driver {
        if is_pci {
            if let Some(irq) = interrupt_number {
                arch::enable_device_interrupt_irq(irq);
            } else {
                kwarn!(
                    "`{}` has no PCI IRQ; device will not receive interrupts",
                    driver.device_name()
                );
            }
        }
        register_found_driver(driver, interrupt_number);
    }
}

pub fn register_matched_driver(driver: Arc<dyn DriverOps>) {
    let name = driver.device_name();
    DRIVERS.write().insert(name, driver);
}

/// Register `driver` as the handler for `irq`. Used when a device's
/// interrupt number is discovered outside the usual `found_device` flow.
pub fn register_irq_handler(irq: u32, driver: Arc<dyn DriverOps>) {
    INTERRUPT_MAP.write().insert(irq, driver);
}

pub fn get_block_driver(name: &str) -> Option<Arc<dyn BlockDriverOps>> {
    DRIVERS
        .read()
        .get(name)
        .cloned()
        .and_then(|driver| driver.as_block_driver())
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
