use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::config;
use crate::kernel::errno::SysResult;
use crate::klib::SleepLock;

use super::{Index, InodeOps};

pub struct Cache {
    cache: SleepLock<BTreeMap<Index, Arc<dyn InodeOps>>>,
}

impl Cache {
    pub const fn new() -> Self {
        Self {
            cache: SleepLock::new(BTreeMap::new(), "InodeCache::cache"),
        }
    }

    pub fn find(&self, index: &Index) -> Option<Arc<dyn InodeOps>> {
        self.cache.lock().get(index).cloned()
    }

    pub fn get_or_insert(&self, index: Index, inode: Arc<dyn InodeOps>) -> Arc<dyn InodeOps> {
        let mut cache = self.cache.lock();

        if let Some(existing) = cache.get(&index) {
            return existing.clone();
        }

        if cache.len() >= config::INODE_CACHE_SIZE {
            cache.retain(|_, inode| Arc::strong_count(inode) > 1);
            if cache.len() >= config::INODE_CACHE_SIZE {
                return inode;
            }
        }

        cache.insert(index, inode.clone());
        inode
    }

    pub fn remove(&self, index: &Index) -> Option<Arc<dyn InodeOps>> {
        self.cache.lock().remove(index)
    }

    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    pub fn prune_unused(&self) -> usize {
        let mut cache = self.cache.lock();
        let v: Vec<_> = cache.extract_if(.., |_, inode| Arc::strong_count(inode) <= 1).collect();
        drop(cache);

        v.len()
    }

    pub fn sync(&self) -> SysResult<()> {
        let inodes: Vec<Arc<dyn InodeOps>> = self.cache.lock().values().cloned().collect();
        for inode in inodes {
            inode.sync()?;
        }
        Ok(())
    }
}
