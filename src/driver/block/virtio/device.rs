use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::block::{BlockDevice, BlockDriver};

use super::{VirtIOBlockDriverInner, VirtIOBlockDriver};

pub struct VirtIOBlockDevice {
    driver: Arc<VirtIOBlockDriverInner>,
}

impl VirtIOBlockDevice {
    pub fn new(addr: usize) -> Self {
        Self {
            driver: Arc::new(VirtIOBlockDriverInner::new(addr)),
        }
    }
}

impl BlockDevice for VirtIOBlockDevice {
    fn name(&self) -> String {
        "virtio_blk".into()
    }

    fn driver(&self) -> Box<dyn BlockDriver> {
        Box::new(VirtIOBlockDriver::new(self.driver.clone()))
    }
}
