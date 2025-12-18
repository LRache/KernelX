use alloc::format;
use alloc::sync::Arc;
use alloc::string::String;
use num_enum::TryFromPrimitive;

use crate::kernel::event::{PollEventSet, FileEvent};
use crate::kernel::mm::AddrSpace;
use crate::kernel::errno::{SysResult, Errno};
use crate::driver::{DeviceType, CharDriverOps, DriverOps};
use crate::kwarn;

const VINTR:  usize = 0;
const VQUIT:  usize = 1;
const VERASE: usize = 2;
const VEOF:   usize = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
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

    fn as_char_driver(self: Arc<Self>) -> Option<Arc<dyn CharDriverOps>> {
        Some(self)
    }
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
enum IOCTLReq {
    TCGETS = 0x5401,
    TCSETS = 0x5402,
    TCSETSW = 0x5403,
    TCSETSF = 0x5404,
    TIOCGWINSZ = 0x5413
}

impl CharDriverOps for Tty {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.driver.read(buf)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.driver.write(buf)
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        self.driver.wait_event(waker, event)
    }

    fn wait_event_cancel(&self) {
        self.driver.wait_event_cancel();
    }

    // TODO: Implement termios settings.
    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let req = IOCTLReq::try_from(request).map_err(|_| Errno::EINVAL)?;
        match req {
            IOCTLReq::TIOCGWINSZ => {
                // Return a default window size of 80x25.
                let winsize = WinSize {
                    ws_row: 25,
                    ws_col: 80,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                addrspace.copy_to_user(arg, winsize)?;
                Ok(0)
            },
            IOCTLReq::TCGETS => {
                let mut termios = Termios::default();
                termios.c_cc[VINTR]  = 0x03; // Ctrl-C
                termios.c_cc[VQUIT]  = 0x1c; // Ctrl-
                termios.c_cc[VERASE] = 0x7f; // DEL
                termios.c_cc[VEOF]   = 0x04; // Ctrl-D
                addrspace.copy_to_user(arg, termios)?;
                Ok(0)
            },
            IOCTLReq::TCSETS => {
                let _termios = addrspace.copy_from_user::<Termios>(arg)?;
                // kinfo!("set termios: {:?}", _termios);
                Ok(0)
            }
            _ => {
                kwarn!("tty ioctl request {:?} not implemented", req);
                Ok(0)
            }
        }
    }
}
