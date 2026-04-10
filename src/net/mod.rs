pub mod interface;
pub mod manager;
pub mod protocol;
pub mod socket;

/// Early init: register loopback. Called before interrupts are enabled.
pub fn init() {
    manager::init();
}

/// Late init: DHCP etc. Must be called from a kernel thread with interrupts enabled.
pub fn configure() {
    for iface in manager::list() {
        if !iface.is_loopback() {
            if let Err(e) = iface.dhcp_configure() {
                crate::kwarn!("DHCP failed on {}: {:?}", iface.name(), e);
            }
        }
    }
}
