mod device;
mod driver;
mod manager;
mod matcher;

pub mod block;
pub mod char;
pub mod chosen;
pub mod net;
mod pci;
pub mod pmu;
pub mod rtc;
pub mod virtio;

pub use device::{Device, DeviceType, PCIDevice, PciInterrupt};
pub use driver::*;
pub use matcher::{MMIOMatcher, PCIMatcher};

pub use manager::{found_device, get_block_driver, handle_interrupt, register_matched_driver};
// pub use fdt::load_device_tree;

#[unsafe(link_section = ".text.init")]
pub fn init() {
    matcher::register_matchers();
}
