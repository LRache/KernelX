use alloc::format;
use alloc::sync::Arc;
use alloc::string::String;
use num_enum::TryFromPrimitive;

use crate::kernel::event::{PollEventSet, FileEvent};
use crate::kernel::errno::{SysResult, Errno};
use crate::driver::{DeviceType, CharDriverOps, DriverOps};
use crate::kernel::mm::AddrSpace;

#[repr(C)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

impl Default for Termios {
    fn default() -> Self {
        Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 32],
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }
}

pub struct Tty {
    index: u32,
    driver: Arc<dyn CharDriverOps>
}

impl Tty {
    pub fn new(index: u32, driver: Arc<dyn CharDriverOps>) -> Self {
        Tty { index, driver }
    }
}

impl DriverOps for Tty {
    fn name(&self) -> &str {
        "tty"
    }

    fn device_name(&self) -> String {
        format!("tty{}", self.index)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn as_char_driver(self: Arc<Self>) -> Arc<dyn CharDriverOps> {
        self
    }
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
enum IOCTLReq {
    TCGETS = 0x5401,
    TCSETS = 0x5402,
}

impl CharDriverOps for Tty {
    fn putchar(&self, c: u8) {
        self.driver.putchar(c);
    }

    fn getchar(&self) -> Option<u8> {
        self.driver.getchar()
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        self.driver.wait_event(waker, event)
    }

    fn wait_event_cancel(&self) {
        self.driver.wait_event_cancel();
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let req = IOCTLReq::try_from(request).map_err(|_| Errno::EINVAL)?;
        // match req {
            
        // }
        Ok(0)
    }
}
