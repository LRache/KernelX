use core::ptr::NonNull;
use spin::Mutex;
use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{VirtIOHeader, MmioTransport};

use crate::driver::virtio::VirtIOHal;

pub struct VirtIOBlockDriverInner {
    driver: Mutex<VirtIOBlk<VirtIOHal, MmioTransport>>,
}

impl VirtIOBlockDriverInner {
    pub fn new(addr: usize) -> Self {
        let addr_ptr = addr as *mut usize;
        let transport = unsafe {
            MmioTransport::new(NonNull::new(addr_ptr as *mut VirtIOHeader).unwrap())
                .expect("Failed to create MMIO transport for VirtIO block driver")
        };
        Self {
            driver: Mutex::new(VirtIOBlk::new(transport).unwrap()),
        }
    }

    pub fn read_blocks(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.driver.lock().read_blocks(block, buf).map_err(|_| ())
    }

    pub fn write_blocks(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        self.driver.lock().write_blocks(block, buf).map_err(|_| ())
    }

    pub fn capacity(&self) -> u64 {
        self.driver.lock().capacity()
    }
}
