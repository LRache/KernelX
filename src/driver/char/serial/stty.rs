use core::usize;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{DriverOps, CharDriverOps, DeviceType};
use crate::driver::char::serial::SerialOps;
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::ipc::{KSiFields, SiCode, signum};
use crate::kernel::mm::AddrSpace;
use crate::kernel::event::{Event, FileEvent, PollEventSet, WaitQueue};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::termios::{InputFlags, LocalFlags, OutputFlags, Termios};
use crate::klib::SpinLock;
use crate::klib::ring::RingBuffer;

struct LineBuffer<const N: usize> {
    buffer: [u8; N],
    length: usize,
}

impl<const N: usize> LineBuffer<N> {
    fn new() -> Self {
        LineBuffer {
            buffer: [0; N],
            length: 0,
        }
    }

    fn input_char(&mut self, c: u8) -> &mut Self {
        self.buffer[self.length] = c;
        self.length += 1;
        self
    }

    fn delete_char(&mut self) {
        if self.length > 0 {
            self.length -= 1;
        }
    }

    fn move_to_ring_buffer<const M: usize>(&mut self, ring: &mut RingBuffer<u8, M>) {
        for i in 0..self.length {
            ring.push(self.buffer[i]);
        }
        self.length = 0;
    }

    fn empty(&self) -> bool {
        self.length == 0
    }
}

struct Attr {
    icrnl: bool, // Map '\r' to '\n'
    inlcr: bool, // Map '\n' to '\r'
    igncr: bool, // Ignore '\r'
    ocrnl: bool, // Map '\r' to '\n' when outputting
    onlcr: bool, // Map '\n' to '\r\n' when outputting
    opost: bool,
    echo: bool,  // output input characters
    echoe: bool, // erase character echo back as BS SP BS
    canonical: bool,
}

impl Default for Attr {
    fn default() -> Self {
        Attr {
            icrnl: true,
            inlcr: false,
            igncr: false,
            ocrnl: false,
            onlcr: false,
            opost: true,
            echo: true,
            echoe: true,
            canonical: true,
        }
    }
}

pub struct Stty {
    name: String,
    serial: SpinLock<Box<dyn SerialOps>>,
    recv_buffer: SpinLock<RingBuffer<u8, 1024>>,
    line: SpinLock<LineBuffer<1024>>,
    waiters: SpinLock<WaitQueue<Event>>,
    winsize: SpinLock<(u16, u16)>, // (rows, cols)

    attr: SpinLock<Attr>,
}

impl Stty {
    pub fn new(name: String, serial: Box<dyn SerialOps>) -> Self {
        Stty {
            name,
            serial: SpinLock::new(serial),
            recv_buffer: SpinLock::new(RingBuffer::new(0)),
            line: SpinLock::new(LineBuffer::new()),
            waiters: SpinLock::new(WaitQueue::new()),
            winsize: SpinLock::new((25, 80)),

            attr: SpinLock::new(Attr::default()),
        }
    }

    fn set_termios(&self, termios: &Termios) {
        let mut attr = self.attr.lock();
        attr.icrnl = termios.c_iflag.contains(InputFlags::ICRNL);
        attr.inlcr = termios.c_iflag.contains(InputFlags::INLCR);
        attr.ocrnl = termios.c_oflag.contains(OutputFlags::OCRNL);
        attr.onlcr = termios.c_oflag.contains(OutputFlags::ONLCR);
        attr.opost = termios.c_oflag.contains(OutputFlags::OPOST);
        attr.echo = termios.c_lflag.contains(LocalFlags::ECHO);
        attr.canonical = termios.c_lflag.contains(LocalFlags::ICANON);
    }
}

#[inline(always)]
fn putchar_helper(serial: &mut Box<dyn SerialOps>, c: u8, onlcr: bool, ocrnl: bool) {
    match c {
        b'\r' => {
            if ocrnl {
                while !serial.putchar(b'\n') {}
            } else {
                while !serial.putchar(b'\r') {}
            }
        }
        b'\n' => {
            if onlcr {
                while !serial.putchar(b'\r') {}
            }
            while !serial.putchar(b'\n') {}
        }
        _ => {
            while !serial.putchar(c) {}
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
            let c = match c {
                b'\r' => {
                    if attr.igncr {
                        continue;
                    }
                    if attr.icrnl {
                        b'\n'
                    } else {
                        b'\r'
                    }
                },
                b'\n' => {
                    if attr.inlcr {
                        b'\r'
                    } else {
                        b'\n'
                    }
                },
                _ => c,
            };

            let mut push_to_buffer = true;
            match c {
                0x7f => { // DEL
                    let mut line = self.line.lock();
                    if attr.echoe {
                        if attr.canonical {
                            if !line.empty() {
                                serial.putchar(0x08); // Backspace
                                serial.putchar(b' '); // Space
                                serial.putchar(0x08); // Backspace
                            }
                            push_to_buffer = false;
                        } else {
                            serial.putchar(0x08); // Backspace
                        }
                    } else {
                        serial.putchar(0x08); // Backspace
                    }

                    if attr.canonical {
                        line.delete_char();
                    }
                }
                0x3 => { // Ctrl-C
                    if attr.canonical {
                        if current::has_task() {
                            let _ = current::pcb().send_signal(signum::SIGQUIT, SiCode::EMPTY, KSiFields::Empty, None);
                        }
                        push_to_buffer = false;
                    }
                    if attr.echo {
                        serial.putchar(b'^');
                        serial.putchar(b'C');
                    }
                }
                0x4 => { // Ctrl-D
                    if attr.echo {
                        serial.putchar(b'^');
                        serial.putchar(b'D');
                    }
                    if attr.canonical {
                        self.line.lock().move_to_ring_buffer(&mut *recv_buffer);
                        recv_buffer.push(0x4); // EOF
                    }
                }
                b'\n' => {
                    if attr.echo {
                        serial.putchar(b'\n');
                    }
                    if attr.canonical {
                        self.line.lock().input_char(b'\n')
                                        .move_to_ring_buffer(&mut *recv_buffer);
                    }
                }
                _ => {
                    if attr.echo {
                        serial.putchar(c);
                    }
                    if attr.canonical {
                        self.line.lock().input_char(c);
                    }
                },
            };

            if !attr.canonical && push_to_buffer {
                recv_buffer.push(c);
            }
        }
        drop(recv_buffer);

        self.waiters.lock().wake_all(|e| e);
    }
}

impl CharDriverOps for Stty {
    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut serial = self.serial.lock();
        let attr = self.attr.lock();
        let onlcr = attr.onlcr & attr.opost;
        let ocrnl = attr.ocrnl & attr.opost;
        for &c in buf {
            putchar_helper(&mut serial, c, onlcr, ocrnl);
        }
        Ok(buf.len())
    }

    fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        if blocked {
            loop {
                {
                    let mut read = 0;
                    let mut recv_buffer = self.recv_buffer.lock();
                    let attr = self.attr.lock();
                    
                    for i in 0..buf.len() {
                        if let Some(mut c) = recv_buffer.pop() {
                            if attr.icrnl && c == b'\r' {
                                c = b'\n';
                            } else if attr.inlcr && c == b'\n' {
                                c = b'\r';
                            } else if attr.canonical && c == 0x4 { // EOF
                                return Ok(read);
                            }
                            buf[i] = c;
                            read += 1;
                        } else {
                            break;
                        }
                    }

                    if read > 0 {
                        return Ok(read);
                    }
                }
                
                self.waiters.lock().wait_current(Event::ReadReady);
                match current::block("read_stty") {
                    Event::ReadReady => {},
                    Event::Signal => return Err(Errno::EINTR),
                    _ => unreachable!(),
                }
            }
        } else {
            let mut recv_buffer = self.recv_buffer.lock();
            let mut read = 0;

            let attr = self.attr.lock();
            
            for i in buf.iter_mut() {
                if let Some(mut c) = recv_buffer.pop() {
                    if attr.icrnl && c == b'\r' {
                        c = b'\n';
                    } else if attr.inlcr && c == b'\n' {
                        c = b'\r';
                    }
                    *i = c;
                    read += 1;
                } else {
                    break;
                }
            }

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
            TIOCSWINSZ = 0x5414,
            TCGETS2 = 0x802C542A,
        }

        let req = IOCTLReq::try_from(request).map_err(|_| Errno::EINVAL)?;
        match req {
            IOCTLReq::TCGETS => {
                let attr = self.attr.lock();
                let mut termios = Termios::default();
                      
                termios.c_iflag |= InputFlags::IUTF8;
                if attr.icrnl { termios.c_iflag |= InputFlags::ICRNL; }
                if attr.inlcr { termios.c_iflag |= InputFlags::INLCR; }
                if attr.ocrnl { termios.c_oflag |= OutputFlags::OCRNL; }
                if attr.onlcr { termios.c_oflag |= OutputFlags::ONLCR; }
                if attr.opost { termios.c_oflag |= OutputFlags::OPOST; }
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
                self.set_termios(&termios);

                Ok(0)
            }
            IOCTLReq::TCSETSF => {
                let termios = addrspace.copy_from_user::<Termios>(arg)?;
                self.recv_buffer.lock().clear();
                self.set_termios(&termios);
                Ok(0)
            }
            IOCTLReq::TIOCGWINSZ => {
                let (ws_row, ws_col) = *self.winsize.lock();
                let winsize = WinSize {
                    ws_row,
                    ws_col,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                addrspace.copy_to_user(arg, winsize)?;
                Ok(0)
            }
            IOCTLReq::TIOCSWINSZ => {
                let winsize = addrspace.copy_from_user::<WinSize>(arg)?;
                let mut ws = self.winsize.lock();
                *ws = (winsize.ws_row, winsize.ws_col);
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
