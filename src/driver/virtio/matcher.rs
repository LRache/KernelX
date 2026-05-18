use alloc::sync::Arc;
use core::ptr::NonNull;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

use crate::arch;
use crate::driver::block::VirtIOBlockDriver;
use crate::driver::char::serial::virtconsole;
use crate::driver::net::VirtioNetDriver;
use crate::driver::{Device, DriverMatcher, DriverOps};
use crate::net::interface::Interface;

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["virtio,mmio"])?;

        let (mmio_base, mmio_size) = device.mmio()?;

        // Uncached kernel-visible mirror of the MMIO region. RISC-V installs
        // a page-table mapping under the hood; LoongArch returns the DMW0
        // (strongly-ordered) window directly.
        let kbase = arch::mmio_phys_to_kaddr(mmio_base, mmio_size);

        let transport = unsafe { MmioTransport::new(NonNull::new(kbase as *mut VirtIOHeader).unwrap()).ok() }?;

        if let Some(irq) = device.interrupt_number() {
            arch::enable_device_interrupt_irq(irq);
        }

        match transport.device_type() {
            DeviceType::Block => Some(Arc::new(VirtIOBlockDriver::new(device.name().into(), transport))),
            DeviceType::Console => Some(virtconsole::new_driver(virtconsole::device_name(), transport)),
            DeviceType::Network => {
                let net_driver = Arc::new(VirtioNetDriver::new(transport));
                let iface = Arc::new(Interface::new(device.name().into(), net_driver));
                crate::net::manager::register(iface.clone());
                Some(iface)
            }
            _ => None,
        }
    }
}
