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

    fn idle_refcount(inode: &Arc<dyn InodeOps>) -> usize {
        1 + inode.filesystem_refcount_bias()
    }

    fn remove_if(&self, mut predicate: impl FnMut(&Index, &Arc<dyn InodeOps>) -> bool) -> usize {
        let removed = {
            let mut cache = self.cache.lock();
            let reclaimable: Vec<Index> = cache
                .iter()
                .filter_map(|(index, inode)| predicate(index, inode).then_some(*index))
                .collect();
            reclaimable
                .into_iter()
                .filter_map(|index| cache.remove(&index))
                .collect::<Vec<_>>()
        };

        let count = removed.len();
        drop(removed);
        count
    }

    // Return removed inodes so Drop runs after InodeCache::cache is unlocked.
    fn reclaim(cache: &mut BTreeMap<Index, Arc<dyn InodeOps>>) -> Vec<Arc<dyn InodeOps>> {
        let reclaimable: Vec<Index> = cache
            .iter()
            .filter_map(|(index, inode)| (Arc::strong_count(inode) <= Self::idle_refcount(inode)).then_some(*index))
            .collect();
        let mut removed = reclaimable
            .into_iter()
            .filter_map(|index| cache.remove(&index))
            .collect::<Vec<_>>();

        while cache.len() >= config::INODE_CACHE_SIZE {
            let Some(index) = cache.keys().next().copied() else {
                break;
            };
            if let Some(inode) = cache.remove(&index) {
                removed.push(inode);
            }
        }

        removed
    }

    pub fn find(&self, index: &Index) -> Option<Arc<dyn InodeOps>> {
        self.cache.lock().get(index).cloned()
    }

    pub fn get_or_insert(&self, index: Index, inode: Arc<dyn InodeOps>) -> Arc<dyn InodeOps> {
        let removed = {
            let mut cache = self.cache.lock();
            if let Some(existing) = cache.get(&index) {
                return existing.clone();
            }

            let removed = if cache.len() >= config::INODE_CACHE_SIZE {
                Self::reclaim(&mut cache)
            } else {
                Vec::new()
            };
            cache.insert(index, inode.clone());
            removed
        };
        drop(removed);
        inode
    }

    pub fn insert(&self, index: &Index, inode: Arc<dyn InodeOps>) -> SysResult<()> {
        let removed = {
            let mut cache = self.cache.lock();
            let mut removed = Vec::new();

            if !cache.contains_key(index) && cache.len() >= config::INODE_CACHE_SIZE {
                removed = Self::reclaim(&mut cache);
            }

            if let Some(old) = cache.insert(*index, inode) {
                removed.push(old);
            }

            removed
        };
        drop(removed);
        Ok(())
    }

    pub fn remove(&self, index: &Index) {
        let removed = {
            let mut cache = self.cache.lock();
            cache.remove(index)
        };
        drop(removed);
    }

    pub fn clear(&self) {
        let removed = {
            let mut cache = self.cache.lock();
            core::mem::take(&mut *cache)
        };
        drop(removed);
    }

    pub fn prune_unused(&self) -> usize {
        self.remove_if(|_, inode| Arc::strong_count(inode) <= Self::idle_refcount(inode))
    }

    pub fn remove_superblock(&self, sno: u32) -> usize {
        self.remove_if(|index, _| index.sno == sno)
    }

    pub fn superblock_busy(&self, sno: u32) -> bool {
        self.cache
            .lock()
            .iter()
            .any(|(index, inode)| index.sno == sno && Arc::strong_count(inode) > Self::idle_refcount(inode))
    }

    pub fn sync(&self) -> SysResult<()> {
        let inodes: Vec<Arc<dyn InodeOps>> = self.cache.lock().values().cloned().collect();
        for inode in inodes {
            inode.sync()?;
        }
        Ok(())
    }
}
