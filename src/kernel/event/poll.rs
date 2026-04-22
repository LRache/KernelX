use bitflags::bitflags;

bitflags! {
    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FileEvent: i16 {
        const READ_READY  = 0x0001;
        const PRIORITY    = 0x0002;
        const WRITE_READY = 0x0004;
        const ERROR       = 0x0008;
        const HANG_UP     = 0x0010;
        const INVALID     = 0x0020;
    }
}
