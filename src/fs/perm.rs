use alloc::vec::Vec;
use bitflags::bitflags;

use crate::kernel::scheduler::current;
use crate::kernel::uapi::Uid;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PermFlags: u8 {
        const R = 1 << 0;
        const W = 1 << 1;
        const X = 1 << 2;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Perm {
    pub uid: Uid,
    pub gid: Uid,
    pub supplementary_gids: Vec<Uid>,
    pub flags: PermFlags,
}

impl Perm {
    pub fn new(uid: Uid, gid: Uid, flags: PermFlags) -> Self {
        Self {
            uid,
            gid,
            supplementary_gids: Vec::new(),
            flags,
        }
    }

    pub fn current(flags: PermFlags) -> Self {
        let pcb = current::pcb();
        Self {
            uid: pcb.euid(),
            gid: pcb.egid(),
            supplementary_gids: pcb.supplementary_gids(),
            flags,
        }
    }

    pub fn in_group(&self, gid: Uid) -> bool {
        self.gid == gid || self.supplementary_gids.contains(&gid)
    }

    pub fn is_root(&self) -> bool {
        self.uid == 0
    }
}
