use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;

use super::inode::MemoryFileSystemInode;
use crate::fs::inode::{Inode, InodeNumber};
use crate::fs::filesystem::SuperBlock;
use crate::kernel::errno::Errno;

struct Entry {
    pub inode: MemoryFileSystemInode,
    pub entries: BTreeMap<String, InodeNumber>,
}

pub struct MemoryFileSystemSuperBlock {
    fsno: usize,
    entries: Mutex<Vec<Entry>>,
}

unsafe extern "C" {
    static __bin_resource_start: u8;
    static __bin_resource_end  : u8;
}

impl MemoryFileSystemSuperBlock {
    pub fn new(fsno: usize) -> Arc<Self> {
        let this = Arc::new(MemoryFileSystemSuperBlock {
            fsno,
            entries: Mutex::new(Vec::new()),
        });

        let mut entries = this.entries.lock();

        entries.push(Entry {
            inode: MemoryFileSystemInode {
                ino: 0,
                superblock: Arc::downgrade(&this),
                start: 0 as *mut u8,
                size: 0,
            },
            entries: BTreeMap::new(),
        });
        unsafe {
            entries.push(Entry {
                inode: MemoryFileSystemInode {
                    ino: 1,
                    superblock: Arc::downgrade(&this),
                    start: &__bin_resource_start as *const u8 as *mut u8,
                    size: &__bin_resource_end as *const u8 as usize - &__bin_resource_start as *const u8 as usize,
                },
                entries: BTreeMap::new(),
            });
        }
        
        entries[0].entries.insert(String::from("init"), 1);

        drop(entries);

        this
    }

    pub fn get_fsno(&self) -> usize {
        self.fsno
    }

    pub fn lookup(&self, ino: InodeNumber, name: &str) -> Option<InodeNumber> {
        let entries = self.entries.lock();
        if let Some(entry) = entries.get(ino) {
            entry.entries.get(name).cloned()
        } else {
            None
        }
    }
}

impl SuperBlock for MemoryFileSystemSuperBlock {
    fn get_root_inode(&self) -> Box<dyn Inode> {
        Box::new(self.entries.lock()[0].inode.clone()) as Box<dyn Inode>
    }

    fn get_inode(&self, ino: usize) -> Result<Box<dyn Inode>, Errno> {
        self.entries.lock().get(ino).map(|entry| {
            Box::new(entry.inode.clone()) as Box<dyn Inode>
        }).ok_or(Errno::ENOENT)
    }
}
