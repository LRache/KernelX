use alloc::collections::BTreeMap;

use crate::fs::inode;

pub struct Manager {
    mounts: BTreeMap<inode::Index, inode::Index>,
}

impl Manager {
    pub const fn new() -> Self {
        Manager {
            mounts: BTreeMap::new(),
        }
    }

    pub fn add_mount(&mut self, sno: u32, ino: u32, mount_sno: u32, mount_ino: u32) {
        self.mounts.insert(
            inode::Index { sno, ino },
            inode::Index {
                sno: mount_sno,
                ino: mount_ino,
            },
        );
    }

    pub fn get_mount(&self, sno: u32, ino: u32) -> Option<inode::Index> {
        self.mounts.get(&inode::Index { sno, ino }).cloned()
    }
}
