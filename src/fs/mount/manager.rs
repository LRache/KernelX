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

    pub fn add_mount(&mut self, fsno: usize, ino: usize, mount_fsno: usize, mount_ino: usize) {
        self.mounts.insert(
            inode::Index { fsno, ino },
            inode::Index {
                fsno: mount_fsno,
                ino: mount_ino,
            },
        );
    }

    pub fn get_mount(&self, fsno: usize, ino: usize) -> Option<inode::Index> {
        self.mounts.get(&inode::Index { fsno, ino }).cloned()
    }
}
