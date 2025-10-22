use core::array;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use device_tree_parser::{DeviceTreeParser, PropertyValue};
use spin::RwLock;

use crate::driver::block::BlockDriver;
use crate::{kwarn, kinfo};

use super::{DriverMatcher, Device, DriverOps, DeviceType};

pub struct DriverManager {
    matchers: RwLock<Vec<&'static dyn DriverMatcher>>,
    block: RwLock<BTreeMap<String, Arc<dyn BlockDriver>>>,
}

impl DriverManager {
    pub const fn new() -> Self {
        DriverManager {
            matchers: RwLock::new(Vec::new()),
            block: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn load_device_tree(&self, fdt: *const u8) -> Result<(), ()> {
        let data = unsafe { core::slice::from_raw_parts(fdt as *const u32, 2) };
        let magic = u32::from_be(data[0]);
        if magic != 0xd00dfeed {
            kwarn!("Invalid device tree magic: 0x{:x}", magic);
            return Err(());
        }
        
        let total_size = u32::from_be(data[1]) as usize;

        let data = unsafe { core::slice::from_raw_parts(fdt, total_size) };

        let parser = DeviceTreeParser::new(data);
        let tree = parser.parse_tree().map_err(|_| ())?;

        self.load_fdt_node(&tree);
        
        Ok(())
    }

    fn load_fdt_node(&self, node: &device_tree_parser::DeviceTreeNode) {
        self.load_fdt_device(node);
        for child in &node.children {
            
            self.load_fdt_node(child);
        }
    }

    fn load_fdt_device(&self, node: &device_tree_parser::DeviceTreeNode) {
        // let addr;
        if node.has_property("reg") {
            match node["reg"].value {
                PropertyValue::U64Array(array) => {
                    
                }

                PropertyValue::U32Array(array) => {
                    kinfo!("U32Array {:?}", array)
                }

                _ => kwarn!("{}", node["reg"].value),
            }
        }
    }

    pub fn register_matcher(&self, matcher: &'static dyn DriverMatcher) {
        self.matchers.write().push(matcher);
    }

    pub fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        for matcher in self.matchers.read().iter() {
            if let Some(driver) = matcher.try_match(device) {
                return Some(driver);
            }
        }
        kwarn!("No driver found for device: {:?}", device);
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

    pub fn register_block_device(&self, driver: Arc<dyn BlockDriver>) {
        self.block.write().insert(driver.device_name(), driver);
    }

    pub fn get_block_driver(&self, name: &str) -> Option<Arc<dyn BlockDriver>> {
        self.block
            .read()
            .get(name)
            .map(|driver| driver.clone())
    }
}

unsafe impl Sync for DriverManager {}
unsafe impl Send for DriverManager {}

static DRIVER_MANAGER: DriverManager = DriverManager::new();

pub fn found_device(device: &Device) {
    DRIVER_MANAGER.register_device(device);
}

pub fn load_device_tree(fdt: *const u8) {
    DRIVER_MANAGER.load_device_tree(fdt).unwrap();
}

pub fn register_matcher(matcher: &'static dyn DriverMatcher) {
    DRIVER_MANAGER.register_matcher(matcher);
}

pub fn get_block_driver(name: &str) -> Option<Arc<dyn BlockDriver>> {
    DRIVER_MANAGER.get_block_driver(name)
}
