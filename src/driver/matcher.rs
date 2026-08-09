use alloc::sync::Arc;

use super::{Device, DriverOps, PCIDevice, block, char, manager, pci, pmu, rtc, virtio};

pub trait MMIOMatcher: Send + Sync {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>>;
}

pub trait PCIMatcher: Send + Sync {
    fn try_match(&self, device: &mut PCIDevice) -> Option<Arc<dyn DriverOps>>;
}

pub fn register_matchers() {
    manager::register_mmio_matcher(&virtio::MMIOMatcher);
    manager::register_mmio_matcher(&char::serial::ns16550a::MMIOMatcher);
    manager::register_mmio_matcher(&block::starfive_mmc::MMIOMatcher);
    manager::register_mmio_matcher(&block::starfive_sdio::MMIOMatcher);
    manager::register_mmio_matcher(&rtc::goldfish::MMIOMatcher);
    manager::register_mmio_matcher(&rtc::ls7a::MMIOMatcher);
    manager::register_mmio_matcher(&pmu::MMIOMatcher);
    manager::register_mmio_matcher(&pci::MMIOMatcher);

    manager::register_pci_matcher(&virtio::PCIMatcher);
}
