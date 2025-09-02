use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;

use super::inode::MemoryFileSystemInode;
use crate::fs::inode::Inode;
use crate::fs::filesystem::SuperBlock;
use crate::kernel::errno::Errno;

struct Entry {
    pub inode: MemoryFileSystemInode,
    pub entries: BTreeMap<String, u32>,
}

pub struct MemoryFileSystemSuperBlock {
    sno: u32,
    entries: Mutex<Vec<Entry>>,
}

unsafe extern "C" {
    static __bin_resource_start: u8;
    static __bin_resource_end  : u8;
}

impl MemoryFileSystemSuperBlock {
    pub fn new(sno: u32) -> Arc<Self> {
        let this = Arc::new(MemoryFileSystemSuperBlock {
            sno,
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

    pub fn get_fsno(&self) -> u32 {
        self.sno
    }

    pub fn lookup(&self, ino: u32, name: &str) -> Option<u32> {
        let entries = self.entries.lock();
        if let Some(entry) = entries.get(ino as usize) {
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

    fn get_inode(&self, ino: u32) -> Result<Box<dyn Inode>, Errno> {
        self.entries.lock().get(ino as usize).map(|entry| {
            Box::new(entry.inode.clone()) as Box<dyn Inode>
        }).ok_or(Errno::ENOENT)
    }
}
