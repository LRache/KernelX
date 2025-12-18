use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{DriverOps, CharDriverOps, DeviceType};
use crate::driver::char::serial::SerialOps;
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::mm::AddrSpace;
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;
use crate::klib::ring::RingBuffer;

pub struct Stty {
    name: String,
    serial: SpinLock<Box<dyn SerialOps>>,
    recv_buffer: SpinLock<RingBuffer<u8, 1024>>,
    waiters: SpinLock<WaitQueue<usize>>,
}

impl Stty {
    pub fn new(name: String, serial: Box<dyn SerialOps>) -> Self {
        Stty {
            name,
            serial: SpinLock::new(serial),
            recv_buffer: SpinLock::new(RingBuffer::new(0)),
            waiters: SpinLock::new(WaitQueue::new()),
        }
    }
}

impl DriverOps for Stty {
    fn name(&self) -> &str {
        "stty"
    }
    
    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn as_char_driver(self: Arc<Self>) -> Option<Arc<dyn CharDriverOps>> {
        Some(self)
    }

    fn handle_interrupt(&self) {
        let mut serial = self.serial.lock();
        let mut recv_buffer = self.recv_buffer.lock();

        while let Some(c) = serial.getchar() {
            recv_buffer.push(c);
        }
        drop(recv_buffer);

        self.waiters.lock().wake_all(|waker| Event::Poll {
            event: FileEvent::ReadReady,
            waker,
        });
    }
}

impl CharDriverOps for Stty {
    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut serial = self.serial.lock();
        for &c in buf {
            serial.putchar(c);
        }
        Ok(buf.len())
    }

    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut recv_buffer = self.recv_buffer.lock();
        let mut read = 0;

        for i in 0..buf.len() {
            if let Some(c) = recv_buffer.pop() {
                buf[i] = c;
                read += 1;
            } else {
                break;
            }
        }

        Ok(read)
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLOUT) {
            return Ok(Some(FileEvent::WriteReady));
        }

        if event.contains(PollEventSet::POLLIN) {
            if self.recv_buffer.lock().empty() {
                self.waiters.lock().wait_current(waker);
                return Ok(None);
            } else {
                return Ok(Some(FileEvent::ReadReady));
            }
        }

        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.waiters.lock().remove(current::task());
    }
    
    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        const VINTR:  usize = 0;
        const VQUIT:  usize = 1;
        const VERASE: usize = 2;
        const VEOF:   usize = 4;

        #[repr(C)]
        #[derive(Clone, Copy, Debug, Default)]
        struct Termios {
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

        #[repr(usize)]
        #[derive(Debug, Clone, Copy, num_enum::TryFromPrimitive)]
        enum IOCTLReq {
            TCGETS = 0x5401,
            TCSETS = 0x5402,
            TCSETSW = 0x5403,
            TCSETSF = 0x5404,
            TIOCGWINSZ = 0x5413,
        }

        let req = IOCTLReq::try_from(request).map_err(|_| Errno::EINVAL)?;
        match req {
            IOCTLReq::TIOCGWINSZ => {
                let winsize = WinSize {
                    ws_row: 25,
                    ws_col: 80,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                addrspace.copy_to_user(arg, winsize)?;
                Ok(0)
            }
            IOCTLReq::TCGETS => {
                let mut termios = Termios::default();
                termios.c_cc[VINTR] = 0x03; // Ctrl-C
                termios.c_cc[VQUIT] = 0x1c; // Ctrl-\
                termios.c_cc[VERASE] = 0x7f; // DEL
                termios.c_cc[VEOF] = 0x04; // Ctrl-D
                addrspace.copy_to_user(arg, termios)?;
                Ok(0)
            }
            IOCTLReq::TCSETS => {
                let _termios = addrspace.copy_from_user::<Termios>(arg)?;
                Ok(0)
            }
            _ => {
                Ok(0)
            }
        }
    }
}
