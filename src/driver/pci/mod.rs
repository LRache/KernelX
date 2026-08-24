pub(super) mod config;
mod host;
mod interrupt;
mod loongson;

use alloc::string::String;
use alloc::sync::Arc;

use super::{Device, DeviceType, DriverOps, MMIOMatcher as MMIOMatcherTrait};

const PCI_HOST_COMPATIBLE: &[&str] = &["pci-host-ecam-generic", "pci-host-cam-generic"];

pub struct MMIOMatcher;

impl MMIOMatcherTrait for MMIOMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(PCI_HOST_COMPATIBLE)?;

        host::scan_bus(device);

        Some(Arc::new(HostDriver {
            device_name: device.name().into(),
        }))
    }
}

struct HostDriver {
    device_name: String,
}

impl DriverOps for HostDriver {
    fn name(&self) -> &str {
        "pci_host_driver"
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Pci
    }
}
