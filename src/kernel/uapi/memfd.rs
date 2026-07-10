use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MemFdCreateFlags: usize {
        const MFD_CLOEXEC = 0x0001;
        const MFD_ALLOW_SEALING = 0x0002;
        const MFD_NOEXEC_SEAL = 0x0008;
        const MFD_EXEC = 0x0010;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FileSealFlags: usize {
        const F_SEAL_SEAL = 0x0001;
        const F_SEAL_SHRINK = 0x0002;
        const F_SEAL_GROW = 0x0004;
        const F_SEAL_WRITE = 0x0008;
        const F_SEAL_FUTURE_WRITE = 0x0010;
        const F_SEAL_EXEC = 0x0020;
    }
}
