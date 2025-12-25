use bitflags::bitflags;

type tc_flag_t = u32;
type cc_t = u8;
type speed_t = u32;

bitflags! {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct InputFlags: tc_flag_t {
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
    pub struct LocalFlags: tc_flag_t {
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
    pub c_oflag: tc_flag_t,
    pub c_cflag: tc_flag_t,
    pub c_lflag: LocalFlags,
    pub c_line: cc_t,
    pub c_cc: [cc_t; 19],
    pub c_ispeed: speed_t,
    pub c_ospeed: speed_t,
}

pub mod cc {
    pub const VINTR:  usize = 0;
    pub const VQUIT:  usize = 1;
    pub const VERASE: usize = 2;
    pub const VEOF:   usize = 4;
}
