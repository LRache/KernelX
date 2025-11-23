use bitflags::bitflags;

use crate::kernel::uapi::Uid;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PermFlags: u8 {
        const R = 1 << 0;
        const W = 1 << 1;
        const X = 1 << 2;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perm {
    pub uid: Uid,
    pub gid: Uid,
    pub flags: PermFlags,
}

impl Perm {
    pub fn new(flags: PermFlags) -> Self {
        Self { uid: 0, gid: 0, flags }
    }

    pub fn dontcare() -> Self {
        Self { uid: 0, gid: 0, flags: PermFlags::empty() }
    }
}
