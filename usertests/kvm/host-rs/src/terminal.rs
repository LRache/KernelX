use std::ffi::c_void;

#[derive(Default)]
pub struct StdinTermiosGuard {
    saved: KernelxTermios,
    enabled: bool,
    saved_status_flags: libc::c_int,
    status_flags_enabled: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KernelxTermios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 8],
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum InputFlag {
    BrkInt = 0o000002,
    InpCk = 0o000020,
    IStrip = 0o000040,
    InlCr = 0o000100,
    IgnCr = 0o000200,
    ICrNl = 0o000400,
}

impl InputFlag {
    fn mask(self) -> u32 {
        self as u32
    }
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum LocalFlag {
    ISig = 0o0000001,
    ICanon = 0o0000002,
    Echo = 0o0000010,
    IExtEn = 0o0002000,
}

impl LocalFlag {
    fn mask(self) -> u32 {
        self as u32
    }
}

#[repr(usize)]
#[derive(Clone, Copy)]
enum TermiosCc {
    VTime = 5,
    VMin = 6,
}

#[repr(usize)]
#[derive(Clone, Copy)]
enum TtyIoctl {
    TcGets = 0x5401,
    TcSets = 0x5402,
}

impl TtyIoctl {
    fn request(self) -> libc::c_ulong {
        self as libc::c_ulong
    }
}

impl StdinTermiosGuard {
    pub fn enable_raw_input(&mut self) {
        if unsafe {
            libc::ioctl(
                libc::STDIN_FILENO,
                TtyIoctl::TcGets.request(),
                &mut self.saved as *mut KernelxTermios as *mut c_void,
            )
        } != 0
        {
            return;
        }

        let mut raw = self.saved;
        raw.c_iflag &= !(InputFlag::BrkInt.mask()
            | InputFlag::InpCk.mask()
            | InputFlag::IStrip.mask()
            | InputFlag::InlCr.mask()
            | InputFlag::IgnCr.mask());
        raw.c_iflag |= InputFlag::ICrNl.mask();
        raw.c_lflag &=
            !(LocalFlag::ICanon.mask() | LocalFlag::Echo.mask() | LocalFlag::ISig.mask() | LocalFlag::IExtEn.mask());
        raw.c_cc[TermiosCc::VMin as usize] = 1;
        raw.c_cc[TermiosCc::VTime as usize] = 0;
        if unsafe {
            libc::ioctl(
                libc::STDIN_FILENO,
                TtyIoctl::TcSets.request(),
                &raw as *const KernelxTermios as *mut c_void,
            )
        } == 0
        {
            self.enabled = true;
        }
    }

    pub fn enable_nonblocking_input(&mut self) {
        let flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL) };
        if flags < 0 {
            return;
        }
        if unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0 {
            self.saved_status_flags = flags;
            self.status_flags_enabled = true;
        }
    }
}

impl Drop for StdinTermiosGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = unsafe {
                libc::ioctl(
                    libc::STDIN_FILENO,
                    TtyIoctl::TcSets.request(),
                    &self.saved as *const KernelxTermios as *mut c_void,
                )
            };
        }
        if self.status_flags_enabled {
            let _ = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, self.saved_status_flags) };
        }
    }
}
