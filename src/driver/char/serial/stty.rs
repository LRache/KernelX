use core::sync::atomic::{AtomicBool, Ordering};
use core::usize;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{DriverOps, CharDriverOps, DeviceType};
use crate::driver::char::serial::SerialOps;
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::mm::AddrSpace;
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::termios::{InputFlags, LocalFlags, Termios};
use crate::klib::SpinLock;
use crate::klib::ring::RingBuffer;

struct Attr {
    crnl: bool, // Map '\r' to '\n'
    nlcr: bool, // Map '\n' to '\r'
    echo: bool,
    echoe: bool,
    canonical: bool,
}

pub struct Stty {
    name: String,
    serial: SpinLock<Box<dyn SerialOps>>,
    recv_buffer: SpinLock<RingBuffer<u8, 1024>>,
    waiters: SpinLock<WaitQueue<Event>>,

    attr: SpinLock<Attr>,
    recieved_line: AtomicBool,
}

impl Stty {
    pub fn new(name: String, serial: Box<dyn SerialOps>) -> Self {
        Stty {
            name,
            serial: SpinLock::new(serial),
            recv_buffer: SpinLock::new(RingBuffer::new(0)),
            waiters: SpinLock::new(WaitQueue::new()),

            attr: SpinLock::new(Attr { 
                crnl: true,
                nlcr: false,
                echo: true,
                echoe: true,
                canonical: true,
            }),
            recieved_line: AtomicBool::new(false),
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
        let attr = self.attr.lock();

        while let Some(c) = serial.getchar() {
            recv_buffer.push(c);
            
            match c {
                b'\r' => {
                    if attr.echo {
                        serial.putchar(b'\n');
                    }
                    self.recieved_line.store(true, Ordering::SeqCst); 
                },
                0x7f  => if attr.echo {
                    if attr.echoe {
                        recv_buffer.pop(); // Delete the character 0x7f

                        serial.putchar(0x08); // Backspace
                        if recv_buffer.pop().is_some() && attr.canonical {
                            serial.putchar(b' ');
                            serial.putchar(0x08);
                        }
                    } else {
                        serial.putchar(0x08); // Backspace
                    }
                }
                _ => {
                    if attr.echo {
                        serial.putchar(c);
                    }
                },
            };
        }
        drop(recv_buffer);

        self.waiters.lock().wake_all(|e| e);
    }
}

impl CharDriverOps for Stty {
    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut serial = self.serial.lock();
        for &c in buf {
            while !serial.putchar(c) {}
        }
        Ok(buf.len())
    }

    fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        let mut helper = || {
            let mut recv_buffer = self.recv_buffer.lock();
            let mut read = 0;

            let attr = self.attr.lock();
            if attr.canonical {
                if self.recieved_line.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                    return 0;
                }
            }
            
            for i in 0..buf.len() {
                if let Some(mut c) = recv_buffer.pop() {
                    if attr.crnl && c == b'\r' {
                        c = b'\n';
                    } else if attr.nlcr && c == b'\n' {
                        c = b'\r';
                    }
                    buf[i] = c;
                    read += 1;
                } else {
                    break;
                }
            }

            read
        };

        if blocked {
            loop {
                let read = helper();
                if read > 0 {
                    return Ok(read);
                }
                self.waiters.lock().wait_current(Event::ReadReady);
                match current::block("read_stty") {
                    Event::ReadReady => {},
                    Event::Signal => return Err(Errno::EINTR),
                    _ => unreachable!(),
                }
            }
        } else {
            let read = helper();
            return Ok(read);
        }
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLOUT) {
            return Ok(Some(FileEvent::WriteReady));
        }

        if event.contains(PollEventSet::POLLIN) {
            if self.recv_buffer.lock().empty() {
                self.waiters.lock().wait_current(Event::Poll { event: FileEvent::ReadReady, waker });
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
            TCGETS2 = 0x802C542A,
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
                let attr = self.attr.lock();
                let mut termios = Termios::default();
                      
                termios.c_iflag |= InputFlags::IUTF8;
                if attr.crnl { termios.c_iflag |= InputFlags::ICRNL; }
                if attr.nlcr { termios.c_iflag |= InputFlags::INLCR; }
                if attr.canonical { termios.c_lflag |= LocalFlags::ICANON; }
                if attr.echo { termios.c_lflag |= LocalFlags::ECHO; }
                
                use crate::kernel::uapi::termios::cc::*;
                termios.c_cc[VINTR] = 0x03; // Ctrl-C
                termios.c_cc[VQUIT] = 0x1c; // Ctrl-\
                termios.c_cc[VERASE] = 0x7f; // DEL
                termios.c_cc[VEOF] = 0x04; // Ctrl-D
                addrspace.copy_to_user(arg, termios)?;
                
                Ok(0)
            }
            IOCTLReq::TCSETS => {
                let termios = addrspace.copy_from_user::<Termios>(arg)?;
                let mut attr = self.attr.lock();
                attr.crnl = termios.c_iflag.contains(InputFlags::ICRNL);
                attr.nlcr = termios.c_iflag.contains(InputFlags::INLCR);
                attr.echo = termios.c_lflag.contains(LocalFlags::ECHO);
                attr.canonical = termios.c_lflag.contains(LocalFlags::ICANON);
                Ok(0)
            }
            IOCTLReq::TCGETS2 => {
                // TODO: implement TCGETS2
                Ok(0)
            }

            _ => {
                Err(Errno::EINVAL)
            }
        }
    }
}
