use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};

use super::{Index, LockedInode};

pub struct Cache {
    cache: Mutex<BTreeMap<Index, Arc<LockedInode>>>,
}

impl Cache {
    pub const fn new() -> Self {
        Self {
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn find(&self, index: &Index) -> Option<Arc<LockedInode>> {
        self.cache.lock().get(index).cloned()
    }

    pub fn insert(&self, index: &Index, inode: Arc<LockedInode>) -> SysResult<()> {
        let mut cache = self.cache.lock();
        if cache.len() >= config::INODE_CACHE_SIZE {
            return Err(Errno::ENOMEM);
        }
        
        cache.insert(*index, inode);
        
        Ok(())
    }

    pub fn clear(&self) {
        for (_, inode) in self.cache.lock().iter() {
            inode.destroy().unwrap();
        }
        self.cache.lock().clear();
    }
}