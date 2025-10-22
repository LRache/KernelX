use alloc::sync::Arc;
use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{Transport, DeviceType};

use crate::driver::block::VirtIOBlockDriver;
use crate::driver::{Device, DriverOps, DriverMatcher};

pub struct VirtIODriverMatcher {
    block_count: AtomicU32,
}

impl VirtIODriverMatcher {
    pub const fn new() -> Self {
        Self {
            block_count: AtomicU32::new(0),
        }
    }
}

impl DriverMatcher for VirtIODriverMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        if device.name() != "virtio_mmio" || device.compatible() != "virtio,mmio" {
            return None;
        }
        
        let transport = unsafe {
            MmioTransport::new(NonNull::new(device.mmio_base() as *mut VirtIOHeader).unwrap()).ok()
        }?;

        match transport.device_type() {
            DeviceType::Block => {
                Some(Arc::new(VirtIOBlockDriver::new(self.block_count.load(core::sync::atomic::Ordering::Relaxed), transport)))
            }
            _ => None,
        }
    }
}
