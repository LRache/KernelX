use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use spin::RwLock;

use crate::driver::DriverOps;
use crate::net::interface::Interface;

static INTERFACES: RwLock<BTreeMap<String, Arc<Interface>>> = RwLock::new(BTreeMap::new());

pub fn init() {
    register(Arc::new(Interface::loopback()));
}

pub fn register(iface: Arc<Interface>) {
    let name = iface.device_name();
    INTERFACES.write().insert(name, iface);
}

pub fn get(name: &str) -> Option<Arc<Interface>> {
    INTERFACES.read().get(name).cloned()
}

pub fn list() -> Vec<Arc<Interface>> {
    INTERFACES.read().values().cloned().collect()
}

/// Return the first non-loopback interface, or loopback if none.
pub fn default_interface() -> Option<Arc<Interface>> {
    let ifaces = INTERFACES.read();
    ifaces
        .values()
        .find(|i| !i.is_loopback())
        .or_else(|| ifaces.values().next())
        .cloned()
}

/// Find an interface whose IPv4 address matches, or fall back to default.
pub fn find_interface_for(ip: Ipv4Addr) -> Option<Arc<Interface>> {
    if ip.is_unspecified() {
        return default_interface();
    }
    let ifaces = INTERFACES.read();
    ifaces.values().find(|i| i.ipv4() == Some(ip)).cloned().or_else(|| {
        ifaces
            .values()
            .find(|i| !i.is_loopback())
            .or_else(|| ifaces.values().next())
            .cloned()
    })
}
