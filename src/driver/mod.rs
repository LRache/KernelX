mod device;
mod driver;
mod manager;
mod matcher;
mod virtio;
// mod fdt;

pub mod block;
pub mod char;
pub mod chosen;

use matcher::DriverMatcher;

pub use device::{Device, DeviceType};
pub use driver::*;

pub use manager::{found_device, get_block_driver, get_char_driver, register_matched_driver};
// pub use fdt::load_device_tree;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    matcher::register_matchers();
}
