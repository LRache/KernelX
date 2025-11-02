use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OpenFlags: usize {
        const O_RDONLY    = 0;
        const O_WRONLY    = 1 << 0;
        const O_RDWR      = 1 << 1;
        const O_CREATE    = 1 << 6;
        const O_EXCL      = 1 << 7;
        const O_NOCTTY    = 1 << 8;
        const O_TRUNC     = 1 << 9;
        const O_APPEND    = 1 << 10;
        const O_NONBLOCK  = 1 << 11;
        const O_DSYNC     = 1 << 12;
        const O_ASYNC     = 1 << 13;
        const O_DIRECT    = 1 << 14;
        const O_LARGEFILE = 1 << 15;
        const O_DIRECTORY = 1 << 16;
        const O_NOFOLLOW  = 1 << 17;
        const O_NOATIME   = 1 << 18;
        const O_CLOEXEC   = 1 << 19;
        const O_SYNC      = (1 << 20) | (1 << 12);
        const O_PATH      = 1 << 21;
        const O_TMPFILE   = (1 << 22) | (1 << 16);
    }
}