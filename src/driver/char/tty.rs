use bitflags::bitflags;
use num_enum::TryFromPrimitive;

use crate::kernel::errno::SysResult;
use crate::kernel::mm::AddrSpace;
use crate::klib::SpinLock;
use crate::klib::ring::RingBuffer;

type TcFlag = u32;
type Cc = u8;
type Speed = u32;
const INPUT_BUFFER_SIZE: usize = 4096;
const LINE_BUFFER_SIZE: usize = 1024;

bitflags! {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct InputFlags: TcFlag {
        const IGNBRK = 0o000001;
        const BRKINT = 0o000002;
        const IGNPAR = 0o000004;
        const PARMRK = 0o000010;
        const INPCK  = 0o000020;
        const ISTRIP = 0o000040;
        const INLCR  = 0o000100;
        const IGNCR  = 0o000200;
        const ICRNL  = 0o000400;
        const IUTF8  = 0o040000;
    }
}

bitflags! {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct OutputFlags: TcFlag {
        const OPOST  = 0o000001;
        const ONLCR  = 0o000004;
        const OCRNL  = 0o000010;
    }
}

bitflags! {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct LocalFlags: TcFlag {
        const ISIG    = 0o0000001;
        const ICANON  = 0o0000002;
        const ECHO    = 0o0000010;
        const ECHONL  = 0o0000020;
        const IEXTEN  = 0o0002000;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Termios {
    c_iflag: InputFlags,
    c_oflag: OutputFlags,
    c_cflag: TcFlag,
    c_lflag: LocalFlags,
    c_line: Cc,
    c_cc: [Cc; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Termios2 {
    c_iflag: InputFlags,
    c_oflag: OutputFlags,
    c_cflag: TcFlag,
    c_lflag: LocalFlags,
    c_line: Cc,
    c_cc: [Cc; 19],
    c_ispeed: Speed,
    c_ospeed: Speed,
}

mod cc {
    pub const VINTR: usize = 0;
    pub const VQUIT: usize = 1;
    pub const VERASE: usize = 2;
    pub const VEOF: usize = 4;
}

#[derive(Clone, Copy)]
pub struct TtyAttr {
    pub icrnl: bool,
    pub inlcr: bool,
    pub igncr: bool,
    pub ocrnl: bool,
    pub onlcr: bool,
    pub opost: bool,
    pub echo: bool,
    pub echoe: bool,
    pub canonical: bool,
}

impl Default for TtyAttr {
    fn default() -> Self {
        Self {
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TtyIoctlResult {
    Handled(usize),
    FlushInput(usize),
    Unsupported,
}

pub enum TtyInputEvent {
    Interrupt,
}

struct TtyLineBuffer<const N: usize> {
    buffer: [u8; N],
    length: usize,
}

impl<const N: usize> TtyLineBuffer<N> {
    fn new() -> Self {
        Self {
            buffer: [0; N],
            length: 0,
        }
    }

    fn input_char(&mut self, c: u8) -> &mut Self {
        if self.length < N {
            self.buffer[self.length] = c;
            self.length += 1;
        }
        self
    }

    fn delete_char(&mut self) {
        if self.length > 0 {
            self.length -= 1;
        }
    }

    fn move_to_ring_buffer(&mut self, ring: &mut RingBuffer<u8, INPUT_BUFFER_SIZE>) {
        for i in 0..self.length {
            ring.push(self.buffer[i]);
        }
        self.length = 0;
    }

    fn clear(&mut self) {
        self.length = 0;
    }

    fn empty(&self) -> bool {
        self.length == 0
    }
}

pub struct TtyState {
    attr: SpinLock<TtyAttr>,
    input: SpinLock<RingBuffer<u8, INPUT_BUFFER_SIZE>>,
    line: SpinLock<TtyLineBuffer<LINE_BUFFER_SIZE>>,
    winsize: SpinLock<(u16, u16)>,
}

impl TtyState {
    pub fn new() -> Self {
        Self {
            attr: SpinLock::new(TtyAttr::default(), "TtyState::attr"),
            input: SpinLock::new(RingBuffer::new(0), "TtyState::input"),
            line: SpinLock::new(TtyLineBuffer::new(), "TtyState::line"),
            winsize: SpinLock::new((25, 80), "TtyState::winsize"),
        }
    }

    pub fn attr(&self) -> TtyAttr {
        *self.attr.lock()
    }

    fn set_termios(&self, termios: &Termios) {
        let mut attr = self.attr.lock();
        attr.icrnl = termios.c_iflag.contains(InputFlags::ICRNL);
        attr.inlcr = termios.c_iflag.contains(InputFlags::INLCR);
        attr.igncr = termios.c_iflag.contains(InputFlags::IGNCR);
        attr.ocrnl = termios.c_oflag.contains(OutputFlags::OCRNL);
        attr.onlcr = termios.c_oflag.contains(OutputFlags::ONLCR);
        attr.opost = termios.c_oflag.contains(OutputFlags::OPOST);
        attr.echo = termios.c_lflag.contains(LocalFlags::ECHO);
        attr.canonical = termios.c_lflag.contains(LocalFlags::ICANON);
    }

    pub fn input_ready(&self) -> bool {
        !self.input.lock().empty()
    }

    pub fn clear_input(&self) {
        self.line.lock().clear();
        self.input.lock().clear();
    }

    pub fn read_input(&self, buf: &mut [u8]) -> Option<usize> {
        let attr = self.attr();
        let mut input = self.input.lock();
        let mut read = 0;

        for i in buf.iter_mut() {
            if let Some(mut c) = input.pop() {
                if attr.icrnl && c == b'\r' {
                    c = b'\n';
                } else if attr.inlcr && c == b'\n' {
                    c = b'\r';
                } else if attr.canonical && c == 0x4 {
                    return Some(read);
                }
                *i = c;
                read += 1;
            } else {
                break;
            }
        }

        if read > 0 { Some(read) } else { None }
    }

    pub fn process_input_byte(&self, byte: u8, mut echo: impl FnMut(u8)) -> Option<TtyInputEvent> {
        let attr = self.attr();
        let mut line = self.line.lock();
        let mut input = self.input.lock();
        let c = match byte {
            b'\r' => {
                if attr.igncr {
                    return None;
                }
                if attr.icrnl { b'\n' } else { b'\r' }
            }
            b'\n' => {
                if attr.inlcr {
                    b'\r'
                } else {
                    b'\n'
                }
            }
            _ => byte,
        };

        let mut event = None;
        let mut push_to_buffer = true;
        match c {
            0x7f => {
                if attr.canonical {
                    if attr.echoe {
                        if !line.empty() {
                            echo(0x08);
                            echo(b' ');
                            echo(0x08);
                        }
                        push_to_buffer = false;
                    } else {
                        echo(0x08);
                    }
                    line.delete_char();
                } else if attr.echo {
                    echo(0x08);
                }
            }
            0x3 => {
                if attr.canonical {
                    event = Some(TtyInputEvent::Interrupt);
                    push_to_buffer = false;
                }
                if attr.echo {
                    echo(b'^');
                    echo(b'C');
                }
            }
            0x4 => {
                if attr.echo {
                    echo(b'^');
                    echo(b'D');
                }
                if attr.canonical {
                    line.move_to_ring_buffer(&mut *input);
                    input.push(0x4);
                }
            }
            b'\n' => {
                if attr.echo {
                    echo(b'\n');
                }
                if attr.canonical {
                    line.input_char(b'\n').move_to_ring_buffer(&mut *input);
                }
            }
            _ => {
                if attr.echo {
                    echo(c);
                }
                if attr.canonical {
                    line.input_char(c);
                }
            }
        }

        if !attr.canonical && push_to_buffer {
            input.push(c);
        }
        event
    }

    pub fn process_output_byte(&self, byte: u8, mut write: impl FnMut(u8)) {
        let attr = self.attr();
        if !attr.opost {
            write(byte);
            return;
        }

        match byte {
            b'\r' if attr.ocrnl => write(b'\n'),
            b'\n' => {
                if attr.onlcr {
                    write(b'\r');
                }
                write(b'\n');
            }
            _ => write(byte),
        }
    }

    pub fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<TtyIoctlResult> {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct WinSize {
            ws_row: u16,
            ws_col: u16,
            ws_xpixel: u16,
            ws_ypixel: u16,
        }

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Termio {
            c_iflag: u16,
            c_oflag: u16,
            c_cflag: u16,
            c_lflag: u16,
            c_line: u8,
            c_cc: [u8; 8],
        }

        #[repr(usize)]
        #[derive(Debug, Clone, Copy, TryFromPrimitive)]
        enum IoctlReq {
            TCGETS = 0x5401,
            TCSETS = 0x5402,
            TCSETSW = 0x5403,
            TCSETSF = 0x5404,
            TCGETA = 0x5405,
            TIOCGWINSZ = 0x5413,
            TIOCSWINSZ = 0x5414,
            TCGETS2 = 0x802C542A,
            TCSETS2 = 0x402C542B,
        }

        let Ok(req) = IoctlReq::try_from(request) else {
            return Ok(TtyIoctlResult::Unsupported);
        };

        match req {
            IoctlReq::TCGETS => {
                addrspace.copy_to_user(arg, self.termios())?;
                Ok(TtyIoctlResult::Handled(0))
            }
            IoctlReq::TCGETA => {
                let termios = self.termios();
                let termio = Termio {
                    c_iflag: termios.c_iflag.bits() as u16,
                    c_oflag: termios.c_oflag.bits() as u16,
                    c_cflag: termios.c_cflag as u16,
                    c_lflag: termios.c_lflag.bits() as u16,
                    c_line: termios.c_line,
                    c_cc: termios.c_cc,
                };
                addrspace.copy_to_user(arg, termio)?;
                Ok(TtyIoctlResult::Handled(0))
            }
            IoctlReq::TCSETS | IoctlReq::TCSETSW => {
                let termios = addrspace.copy_from_user::<Termios>(arg)?;
                self.set_termios(&termios);
                Ok(TtyIoctlResult::Handled(0))
            }
            IoctlReq::TCSETSF => {
                let termios = addrspace.copy_from_user::<Termios>(arg)?;
                self.set_termios(&termios);
                Ok(TtyIoctlResult::FlushInput(0))
            }
            IoctlReq::TIOCGWINSZ => {
                let (ws_row, ws_col) = *self.winsize.lock();
                let winsize = WinSize {
                    ws_row,
                    ws_col,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                addrspace.copy_to_user(arg, winsize)?;
                Ok(TtyIoctlResult::Handled(0))
            }
            IoctlReq::TIOCSWINSZ => {
                let winsize = addrspace.copy_from_user::<WinSize>(arg)?;
                *self.winsize.lock() = (winsize.ws_row, winsize.ws_col);
                Ok(TtyIoctlResult::Handled(0))
            }
            IoctlReq::TCGETS2 => {
                addrspace.copy_to_user(arg, self.termios2())?;
                Ok(TtyIoctlResult::Handled(0))
            }
            IoctlReq::TCSETS2 => {
                let termios = addrspace.copy_from_user::<Termios2>(arg)?;
                self.set_termios2(&termios);
                Ok(TtyIoctlResult::Handled(0))
            }
        }
    }

    fn set_termios2(&self, termios: &Termios2) {
        let termios = Termios {
            c_iflag: termios.c_iflag,
            c_oflag: termios.c_oflag,
            c_cflag: termios.c_cflag,
            c_lflag: termios.c_lflag,
            c_line: termios.c_line,
            c_cc: Default::default(),
        };
        self.set_termios(&termios);
    }

    fn termios(&self) -> Termios {
        let attr = self.attr();
        let mut termios = Termios::default();

        termios.c_iflag |= InputFlags::IUTF8;
        if attr.icrnl {
            termios.c_iflag |= InputFlags::ICRNL;
        }
        if attr.inlcr {
            termios.c_iflag |= InputFlags::INLCR;
        }
        if attr.igncr {
            termios.c_iflag |= InputFlags::IGNCR;
        }
        if attr.ocrnl {
            termios.c_oflag |= OutputFlags::OCRNL;
        }
        if attr.onlcr {
            termios.c_oflag |= OutputFlags::ONLCR;
        }
        if attr.opost {
            termios.c_oflag |= OutputFlags::OPOST;
        }
        if attr.canonical {
            termios.c_lflag |= LocalFlags::ICANON;
        }
        if attr.echo {
            termios.c_lflag |= LocalFlags::ECHO;
        }

        use self::cc::*;
        termios.c_cc[VINTR] = 0x03;
        termios.c_cc[VQUIT] = 0x1c;
        termios.c_cc[VERASE] = 0x7f;
        termios.c_cc[VEOF] = 0x04;
        termios
    }

    fn termios2(&self) -> Termios2 {
        let termios = self.termios();
        Termios2 {
            c_iflag: termios.c_iflag,
            c_oflag: termios.c_oflag,
            c_cflag: termios.c_cflag,
            c_lflag: termios.c_lflag,
            c_line: termios.c_line,
            c_cc: Default::default(),
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }
}
