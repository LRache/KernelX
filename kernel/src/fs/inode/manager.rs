use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use spin::mutex::Mutex;

use crate::fs::inode::{LockedInode, Index};

// pub struct InodeWrapper {
//     inner: Mutex<dyn Inode>,
// }

// unsafe impl Send for InodeWrapper {}
// unsafe impl Sync for InodeWrapper {}

// impl InodeWrapper {
//     pub fn new(inode: Box<dyn Inode>) -> Arc<Self> {
//         Arc::new(InodeWrapper {
//             inner: Mutex::new(inode),
//         })
//     }

//     pub fn new_raw(inode: Box<dyn Inode>) -> Self {
//         Self {
//             inner: Mutex::new(inode),
//         }
//     }

//     pub fn get_fsno(&self) -> usize {
//         self.inner.lock().get_fsno()
//     }

//     pub fn get_ino(&self) -> usize {
//         self.inner.lock().get_ino()
//     }
    
//     pub fn readat(&self, buf: &mut [u8], offset: usize) -> Result<usize, Errno> {
//         self.inner.lock().readat(buf, offset)
//     }

//     pub fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
//         self.inner.lock().writeat(buf, offset)
//     }

//     pub fn lookup(&self, name: &str) -> Result<inode::InodeNumber, Errno> {
//         self.inner.lock().lookup(name)
//     }

//     pub fn size(&self) -> Result<usize, Errno> {
//         self.inner.lock().size()
//     }
// }

pub struct Manager {
    inodes: Mutex<BTreeMap<Index, Weak<LockedInode>>>,
}

impl Manager {
    pub const fn new() -> Self {
        Manager {
            inodes: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn with_inode<F, R>(&self, sno: u32, ino: u32, f: F) -> Option<R>
    where
        F: FnOnce(&LockedInode) -> R,
    {
        if let Some(weak_inode) = self.inodes.lock().get(&Index { sno, ino }) {
            if let Some(strong_inode) = weak_inode.upgrade() {
                return Some(f(&strong_inode));
            }
        }
        None
    }

    pub fn get_inode(&self, sno: u32, ino: u32) -> Option<Arc<LockedInode>> {
        let mut inodes = self.inodes.lock();
        if let Some(weak) = inodes.get(&Index { sno, ino }) {
            if let Some(strong) = weak.upgrade() {
                return Some(strong);
            } else {
                inodes.remove(&Index { sno, ino });
                return None;
            }
        } else {
            None
        }
    }

    pub fn add_inode(&self, sno: u32, ino: u32, inode: LockedInode) -> Arc<LockedInode> {
        let inode = Arc::new(inode);
        self.inodes.lock().insert(
            Index { sno, ino },
            Arc::downgrade(&inode),
        );
        inode
    }
}
