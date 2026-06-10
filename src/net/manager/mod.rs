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

/// Find the interface that owns a specific local IPv4 address.
/// This is for bind()-style local address validation, so subnet matches are
/// intentionally not accepted.
pub fn find_interface_for_local_addr(ip: Ipv4Addr) -> Option<Arc<Interface>> {
    if ip.is_unspecified() {
        return None;
    }
    let ifaces = INTERFACES.read();
    ifaces.values().find(|iface| iface.ipv4() == Some(ip)).cloned()
}

/// Route a packet to the interface that should reach `dst`.
pub fn route_interface_for_dst(dst: Ipv4Addr) -> Option<Arc<Interface>> {
    let ifaces = INTERFACES.read();

    if dst.is_loopback() {
        return ifaces.values().find(|iface| iface.is_loopback()).cloned();
    }

    let mut best_match: Option<(u32, Arc<Interface>)> = None;
    for iface in ifaces.values() {
        if iface.is_loopback() {
            continue;
        }
        let (Some(ip), Some(mask)) = (iface.ipv4(), iface.netmask()) else {
            continue;
        };
        let ip = u32::from(ip);
        let mask = u32::from(mask);
        let dst = u32::from(dst);
        if (ip & mask) != (dst & mask) {
            continue;
        }

        let prefix_len = mask.count_ones();
        if best_match.as_ref().map_or(true, |(best, _)| prefix_len > *best) {
            best_match = Some((prefix_len, iface.clone()));
        }
    }

    best_match
        .map(|(_, iface)| iface)
        .or_else(|| ifaces.values().find(|iface| !iface.is_loopback()).cloned())
        .or_else(|| ifaces.values().next().cloned())
}
