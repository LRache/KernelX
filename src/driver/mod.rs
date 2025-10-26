mod virtio;
mod device;
mod driver;
mod matcher;
mod manager;
// mod fdt;

pub mod block;
pub mod char;
pub mod chosen;

use matcher::DriverMatcher;

pub use device::{Device, DeviceType};
pub use driver::*;

pub use manager::{get_block_driver, get_char_driver, register_matched_driver, found_device};
// pub use fdt::load_device_tree;

#[unsafe(link_section = ".text.init")]
pub fn init() {    
    matcher::register_matchers();
}
