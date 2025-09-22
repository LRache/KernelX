mod virtio;
mod manager;

pub mod block;

pub use manager::DEVICE_MANAGER as MANAGER;

use alloc::boxed::Box;
use crate::driver::block::VirtIOBlockDevice;

pub fn init() {
    MANAGER.register_block_device("virtio_block0", Box::new(VirtIOBlockDevice::new(0x10001000)));
}
