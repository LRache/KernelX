use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use virtio_drivers::device::console::VirtIOConsole;
use virtio_drivers::transport::Transport;

use crate::driver::DriverOps;
use crate::driver::virtio::VirtIOHal;

use super::SerialOps;
use super::stty::Stty;

static CONSOLE_COUNTER: AtomicU32 = AtomicU32::new(0);

struct VirtConsole<T: Transport + Send + 'static> {
    driver: VirtIOConsole<VirtIOHal, T>,
}

impl<T: Transport + Send + 'static> VirtConsole<T> {
    fn new(transport: T) -> Self {
        let driver = VirtIOConsole::new(transport).expect("failed to create VirtIOConsole");
        Self { driver }
    }
}

impl<T: Transport + Send + 'static> SerialOps for VirtConsole<T> {
    fn getchar(&mut self) -> Option<u8> {
        let _ = self.driver.ack_interrupt();
        self.driver.recv(true).ok().flatten()
    }

    fn putchar(&mut self, c: u8) -> bool {
        self.driver.send(c).is_ok()
    }
}

pub fn device_name() -> String {
    let idx = CONSOLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("hvc{}", idx)
}

pub fn new_driver<T: Transport + Send + 'static>(name: String, transport: T) -> Arc<dyn DriverOps> {
    Arc::new(Stty::new(name, Box::new(VirtConsole::new(transport))))
}
