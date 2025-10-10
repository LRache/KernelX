use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OpenFlags: usize {
        const O_RDONLY    = 0x0000;
        const O_WRONLY    = 0x0001;
        const O_RDWR      = 0x0002;
        const O_CREAT     = 0x0040;
        const O_EXCL      = 0x0080;
        const O_NOCTTY    = 0x0100;
        const O_TRUNC     = 0x0200;
        const O_APPEND    = 0x0400;
        const O_NONBLOCK  = 0x0800;
        const O_DSYNC     = 0x1000;
        const FASYNC      = 0x2000;
        const O_DIRECT    = 0x4000;
        const O_LARGEFILE = 0x8000;
        const O_DIRECTORY = 0x10000;
        const O_NOFOLLOW  = 0x20000;
        const O_CLOEXEC   = 0x80000;
    }
}