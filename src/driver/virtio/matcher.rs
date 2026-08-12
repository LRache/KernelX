use alloc::string::String;
use alloc::sync::Arc;
use core::fmt::Write;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::pci::{PciTransport, virtio_device_type};
use virtio_drivers::transport::{DeviceType as VirtioDeviceType, Transport};

use crate::driver::block::VirtIOBlockDriver;
use crate::driver::char::serial::virtconsole;
use crate::driver::net::VirtioNetDriver;
use crate::driver::{Device, DriverOps, MMIOMatcher as MMIOMatcherTrait, PCIDevice, PCIMatcher as PCIMatcherTrait};
use crate::net::interface::Interface;
use crate::{arch, kwarn};

use super::hal::VirtIOHal;
use super::pci::MsixPciTransport;

static PCI_BLOCK_COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct MMIOMatcher;

impl MMIOMatcherTrait for MMIOMatcher {
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
            VirtioDeviceType::Block => Some(VirtIOBlockDriver::new(device.name().into(), transport)),
            VirtioDeviceType::Console => Some(virtconsole::new_driver(virtconsole::device_name(), transport)),
            VirtioDeviceType::Network => {
                let net_driver = Arc::new(VirtioNetDriver::new(transport));
                let iface = Arc::new(Interface::new(device.name().into(), net_driver));
                crate::net::manager::register(iface.clone());
                Some(iface)
            }
            _ => None,
        }
    }
}

pub struct PCIMatcher;

impl PCIMatcherTrait for PCIMatcher {
    fn try_match(&self, device: &mut PCIDevice) -> Option<Arc<dyn DriverOps>> {
        let vdev_type = virtio_device_type(device.info())?;
        let device_function = device.device_function();
        let transport = match PciTransport::new::<VirtIOHal>(device.root(), device_function) {
            Ok(transport) => transport,
            Err(e) => {
                kwarn!("virtio: PciTransport::new failed for {}: {:?}", device_function, e,);
                return None;
            }
        };

        match vdev_type {
            VirtioDeviceType::Block => {
                let name = pci_device_name(vdev_type);
                if device.msix_vector().is_some() {
                    let transport = MsixPciTransport::new(device, transport)?;
                    Some(VirtIOBlockDriver::new(name, transport))
                } else {
                    Some(VirtIOBlockDriver::new(name, transport))
                }
            }
            VirtioDeviceType::Console => {
                let name = virtconsole::device_name();
                if device.msix_vector().is_some() {
                    let transport = MsixPciTransport::new(device, transport)?;
                    Some(virtconsole::new_driver(name, transport))
                } else {
                    Some(virtconsole::new_driver(name, transport))
                }
            }
            VirtioDeviceType::Network => {
                kwarn!("virtio: PCI virtio-net ignored (networking deferred)");
                None
            }
            other => {
                kwarn!("virtio: unsupported virtio-pci device type: {:?}", other);
                None
            }
        }
    }
}

fn pci_device_name(vdev_type: VirtioDeviceType) -> String {
    let mut name = String::new();
    match vdev_type {
        VirtioDeviceType::Block => {
            let idx = PCI_BLOCK_COUNTER.fetch_add(1, Ordering::Relaxed);
            let _ = write!(&mut name, "virtio_block{}", idx);
        }
        VirtioDeviceType::Network => {
            let _ = write!(&mut name, "virtio_net");
        }
        VirtioDeviceType::Console => {
            let _ = write!(&mut name, "{}", virtconsole::device_name());
        }
        other => {
            let _ = write!(&mut name, "virtio_{:?}", other);
        }
    }
    name
}
