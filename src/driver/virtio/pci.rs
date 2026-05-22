use core::mem::size_of;
use core::ptr::{NonNull, addr_of, addr_of_mut};

use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::PCI_CAP_ID_VNDR;
use virtio_drivers::transport::{DeviceStatus, DeviceType, Transport};
use virtio_drivers::{Error, PhysAddr};

use crate::driver::PCIDevice;
use crate::{arch, kwarn};

const CAP_BAR_OFFSET: u16 = 4;
const CAP_BAR_OFFSET_OFFSET: u16 = 8;
const CAP_LENGTH_OFFSET: u16 = 12;
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;

#[repr(C)]
struct VirtioCommonCfg {
    device_feature_select: u32,
    device_feature: u32,
    driver_feature_select: u32,
    driver_feature: u32,
    msix_config: u16,
    num_queues: u16,
    device_status: u8,
    config_generation: u8,
    queue_select: u16,
    queue_size: u16,
    queue_msix_vector: u16,
    queue_enable: u16,
    queue_notify_off: u16,
    queue_desc: u64,
    queue_driver: u64,
    queue_device: u64,
}

struct VirtioCapabilityInfo {
    bar: u8,
    offset: u32,
    length: u32,
}

pub struct MsixPciTransport {
    inner: PciTransport,
    common_cfg: NonNull<VirtioCommonCfg>,
    vector: u16,
}

impl MsixPciTransport {
    pub fn new(device: &mut PCIDevice, inner: PciTransport) -> Option<Self> {
        let vector = device.msix_vector()?;
        let Some(common_cfg) = virtio_common_cfg(device) else {
            kwarn!("virtio: PCI MSI-X enabled but common config was not found");
            return None;
        };
        Some(Self {
            inner,
            common_cfg,
            vector,
        })
    }

    fn set_config_vector(&self) {
        unsafe {
            arch::write_volatile(addr_of_mut!((*self.common_cfg.as_ptr()).msix_config), self.vector);
        }
    }

    fn set_queue_vector(&self, queue: u16) {
        unsafe {
            arch::write_volatile(addr_of_mut!((*self.common_cfg.as_ptr()).queue_select), queue);
            arch::write_volatile(addr_of_mut!((*self.common_cfg.as_ptr()).queue_msix_vector), self.vector);
            let assigned = arch::read_volatile(addr_of!((*self.common_cfg.as_ptr()).queue_msix_vector));
            if assigned != self.vector {
                kwarn!(
                    "virtio: device refused MSI-X vector {} for queue {}",
                    self.vector,
                    queue,
                );
            }
        }
    }
}

unsafe impl Send for MsixPciTransport {}
unsafe impl Sync for MsixPciTransport {}

impl Transport for MsixPciTransport {
    fn device_type(&self) -> DeviceType {
        self.inner.device_type()
    }

    fn read_device_features(&mut self) -> u64 {
        self.inner.read_device_features()
    }

    fn write_driver_features(&mut self, driver_features: u64) {
        self.inner.write_driver_features(driver_features);
    }

    fn max_queue_size(&mut self, queue: u16) -> u32 {
        self.inner.max_queue_size(queue)
    }

    fn notify(&mut self, queue: u16) {
        self.inner.notify(queue);
    }

    fn get_status(&self) -> DeviceStatus {
        self.inner.get_status()
    }

    fn set_status(&mut self, status: DeviceStatus) {
        self.inner.set_status(status);
    }

    fn set_guest_page_size(&mut self, guest_page_size: u32) {
        self.inner.set_guest_page_size(guest_page_size);
    }

    fn requires_legacy_layout(&self) -> bool {
        self.inner.requires_legacy_layout()
    }

    fn queue_set(
        &mut self,
        queue: u16,
        size: u32,
        descriptors: PhysAddr,
        driver_area: PhysAddr,
        device_area: PhysAddr,
    ) {
        self.inner.queue_set(queue, size, descriptors, driver_area, device_area);
        self.set_queue_vector(queue);
    }

    fn queue_unset(&mut self, queue: u16) {
        self.inner.queue_unset(queue);
    }

    fn queue_used(&mut self, queue: u16) -> bool {
        self.inner.queue_used(queue)
    }

    fn ack_interrupt(&mut self) -> bool {
        let _ = self.inner.ack_interrupt();
        true
    }

    fn finish_init(&mut self) {
        self.set_config_vector();
        self.inner.finish_init();
    }

    fn config_space<T: 'static>(&self) -> core::result::Result<NonNull<T>, Error> {
        self.inner.config_space()
    }
}

fn virtio_common_cfg(device: &mut PCIDevice) -> Option<NonNull<VirtioCommonCfg>> {
    let mut common_cfg = None;
    for capability in device.capabilities() {
        if capability.id != PCI_CAP_ID_VNDR {
            continue;
        }
        let cap_len = capability.private_header as u8;
        let cfg_type = (capability.private_header >> 8) as u8;
        if cap_len < 16 || cfg_type != VIRTIO_PCI_CAP_COMMON_CFG {
            continue;
        }

        common_cfg = Some(VirtioCapabilityInfo {
            bar: device.config_read_u32(capability.offset as u16 + CAP_BAR_OFFSET) as u8,
            offset: device.config_read_u32(capability.offset as u16 + CAP_BAR_OFFSET_OFFSET),
            length: device.config_read_u32(capability.offset as u16 + CAP_LENGTH_OFFSET),
        });
        break;
    }

    let common_cfg = common_cfg?;
    if (common_cfg.length as usize) < size_of::<VirtioCommonCfg>() {
        return None;
    }

    let (bar_address, bar_size) = device.bar_address_size(common_cfg.bar)?;
    let common_size = size_of::<VirtioCommonCfg>() as u64;
    if (common_cfg.offset as u64).checked_add(common_size)? > bar_size as u64 {
        return None;
    }

    let common_pa = bar_address.checked_add(common_cfg.offset as u64)?;
    let common_kaddr = arch::mmio_phys_to_kaddr(common_pa as usize, size_of::<VirtioCommonCfg>());
    NonNull::new(common_kaddr as *mut VirtioCommonCfg)
}
