use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct InputFlags: u32 {
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
    #[derive(Clone, Copy, Debug, Default)]
    pub struct LocalFlags: u32 {
        const ISIG    = 0o0000001;
        const ICANON  = 0o0000002;
        const ECHO    = 0o0000010;
        const ECHONL  = 0o0000020;
        const IEXTEN  = 0o0002000;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Termios {
    pub c_iflag: InputFlags,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: LocalFlags,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

pub mod cc {
    pub const VINTR:  usize = 0;
    pub const VQUIT:  usize = 1;
    pub const VERASE: usize = 2;
    pub const VEOF:   usize = 4;
}
