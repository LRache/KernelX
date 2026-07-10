use bitflags::bitflags;

pub mod msg;
pub mod pipe;
pub mod sem;
pub mod shm;
pub mod signal;
pub mod unixsocket;

pub use pipe::Pipe;
pub use signal::*;

pub const IPC_PRIVATE: usize = 0;

bitflags! {
    pub struct IpcGetFlag: usize {
        const IPC_CREAT = 0o1000;
        const IPC_EXCL = 0o2000;
        const IPC_NOWAIT = 0o4000;
    }
}

bitflags! {
    pub struct IpcMode: u32 {
        const WRITE = 0o2;
        const READ = 0o4;
        const ALL = 0o777;
    }
}

bitflags! {
    pub struct IpcCtlFlags: usize {
        const IPC_64 = 0x0100;
    }
}
