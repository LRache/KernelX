use alloc::string::String;

use crate::driver::{DeviceType, DriverOps};

use super::Interface;

impl DriverOps for Interface {
    fn name(&self) -> &str {
        "net_interface"
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }

    fn handle_interrupt(&self) {
        if let Some(drv) = &self.driver {
            let packets = drv.recv_packets();
            for pkt in &packets {
                self.on_receive(pkt);
            }
        }
    }
}
